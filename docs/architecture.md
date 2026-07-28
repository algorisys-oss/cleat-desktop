# Architecture

## Process model

```
┌─────────────────────────────────────────────────────────┐
│ WebView (React + TypeScript)                            │
│   views/ ── api.ts ── invoke() / listen()               │
└──────────────────────────┬──────────────────────────────┘
                           │ Tauri IPC (no socket, no port)
┌──────────────────────────▼──────────────────────────────┐
│ Rust process                                            │
│   commands.rs      46 #[tauri::command] entry points    │
│   state.rs         active runtime + live stream registry│
│   runtime/         dyn ContainerRuntime                 │
│     ├── docker.rs  ┐                                    │
│     └── podman.rs  ┴──> engine.rs (bollard)             │
│         compose.rs ────> argv subprocess                │
└──────────────────────────┬──────────────────────────────┘
                           │ unix socket
                    Docker / Podman daemon
```

The frontend never touches a container daemon. It holds no credentials, opens no
sockets, and knows nothing about bollard. The Rust process owns the only
connection.

This is the single biggest structural change from the Electron version, which ran
an Express server on `0.0.0.0:3000` and had the renderer `fetch()` it. See
[security.md](security.md).

## The runtime abstraction

`ContainerRuntime` ([`runtime/mod.rs`](../tauri-rs/src-tauri/src/runtime/mod.rs)) is
the single interface every command handler talks to — 40 methods covering
containers, images, networks, volumes, exec, and compose.

```rust
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    fn kind(&self) -> RuntimeKind;
    async fn list_containers(&self, all: bool) -> AppResult<Vec<Container>>;
    async fn follow_logs(&self, id: &str, tail: i64) -> AppResult<LogStream>;
    fn compose_argv(&self) -> Vec<String>;
    // ...
}
```

### Why a trait rather than calling bollard directly

Docker and Podman both speak the Engine API, so a naive implementation would call
`bollard` straight from the 46 command handlers. That works right up until the
third runtime, at which point every handler needs a branch.

Instead:

- **`engine.rs`** (~1,000 lines) holds the shared Engine-API client and all the
  model mapping. Both current backends delegate to it.
- **`docker.rs`** and **`podman.rs`** hold only what genuinely differs — socket
  discovery, compose front-end, unsupported operations.
- **Command handlers** are runtime-agnostic. They call
  `state.runtime().await?.some_method()`.

Adding a runtime is additive. See [runtimes.md](runtimes.md#adding-a-runtime).

### DTO boundary

[`model.rs`](../tauri-rs/src-tauri/src/model.rs) defines the types crossing IPC.
They are deliberately flatter than bollard's models: the UI gets exactly the
fields it renders, in camelCase, so daemon schema churn doesn't reach React
components and the TypeScript mirrors in
[`types.ts`](../tauri-rs/src/types.ts) stay small.

Keep the two files in sync — they are hand-mirrored, not generated.

## Streaming

Logs, stats, and image pulls are long-lived streams, not polls. Each follows the
same shape:

1. A command spawns a Tokio task that pumps a runtime stream onto a Tauri event
   channel whose name the frontend supplies.
2. The task handle is registered in `AppState` under a stable key
   (`logs:<id>`, `stats:<id>`, `pull:<image>`) so it can be cancelled.
3. Every channel gets a terminal `<channel>:end` event carrying an optional
   error, so the UI always learns *why* a stream stopped rather than just going
   quiet.

On the frontend, `openStream()` in [`api.ts`](../tauri-rs/src/api.ts) wires the
listeners and returns a single dispose function; views call it from React effect
cleanup. Re-registering the same key aborts the previous task rather than letting
two writers share one channel.

Details that matter in practice:

- **Stats need `one_shot: false`.** Without it the daemon omits `precpu_stats`
  and CPU% cannot be computed at all.
- **CPU deltas use `saturating_sub`.** A counter reset would otherwise underflow
  and render as an absurd spike.
- **Memory subtracts the page cache** (`inactive_file`, falling back to `cache`),
  matching what `docker stats` reports. Without it the number reads far higher
  than the CLI's for the same container.
- **Log buffers are capped** at 5,000 lines in the UI; a chatty container would
  otherwise grow the array unboundedly for as long as the modal is open.
- **Pull progress is aggregated across layers.** Per-layer byte counters are
  summed so the overall bar reflects the whole pull, not whichever layer reported
  last.
- **Compose output is streamed, not awaited.** `compose down` waits a 10-second
  SIGTERM grace period *per* container that ignores it, so a blocking call left
  the UI silent for tens of seconds and read as a hang. `compose::stream()`
  spawns the child with piped stdout+stderr and `kill_on_drop`, so output
  appears within ~70ms and dropping the stream cancels the command. A 15-minute
  deadline is a backstop — generous because `compose up` legitimately pulls
  images.
- **Image copy between runtimes streams too.** The source's `GET
  /images/{name}/get` feeds the destination's `POST /images/load` directly, so a
  multi-gigabyte image never lands on disk. Byte progress is throttled to one
  event per 4 MB; per-chunk emission would push thousands of events per image.

### Exec: the one bidirectional stream

Every stream above is one-way. The interactive terminal is not, and that single
difference drives its design.

The output half is ordinary — a task pumps bytes onto a channel, registered under
`exec:<channel>` like any other. The input half has no precedent: keystrokes
arrive one `exec_write` IPC call at a time and must all reach the *same* writer,
so the write half of the attached connection is parked in `AppState` alongside
the exec id rather than being owned by the pump task.

- **The session key is the channel name**, not the container id. Two terminals
  into one container are legitimate, so the per-container keying used by logs and
  stats would collide.
- **`tty: true` changes the wire format.** The daemon stops multiplexing, so the
  response is raw terminal bytes rather than 8-byte-framed stdout/stderr records.
  The `LogLine` mapping used by log follow is therefore the wrong template — it
  trims newlines and lossily decodes UTF-8, both fatal here.
- **Traffic is base64 in both directions.** Tauri events are JSON: raw bytes
  would serialise as an array of integers (several times larger than base64), and
  a `String` would force a lossy decode that corrupts escape sequences and any
  multi-byte character split across a chunk boundary.
- **Resize is a second round trip.** The size cannot be set at create time, so
  `exec_start` attaches and then calls `resize_exec`. An initial resize failure is
  logged but does not abort the session — a terminal stuck at 80×24 beats no
  terminal. Podman serves this endpoint from its compatibility layer, which is
  why the shared suite pins it on both runtimes.
- **The shell is probed, not assumed.** `/bin/bash` is absent from Alpine and most
  slim images; `detect_shell()` asks the container what it has.

## State

[`state.rs`](../tauri-rs/src-tauri/src/state.rs) holds three things behind
`RwLock`s: the active runtime, the live stream registry, and interactive exec
sessions.

`select_runtime()` connects and pings the *new* runtime before dropping the old
one, so a failed switch leaves the app on a working runtime. Switching also
aborts all live streams, since they belong to the previous daemon.

Exec sessions are registered in *two* places — the pump task in `streams`, the
stdin writer in `execs` — so anything that tears down streams must clear both.
`stop_all_streams()` does; dropping only the task would strand an upgraded socket
against a runtime the app has stopped using. `stop_all_streams_clears_exec_sessions`
pins this.

## Error mapping

[`error.rs`](../tauri-rs/src-tauri/src/error.rs) gives every failure a stable
machine-readable `kind` alongside the daemon's own message:

| kind | meaning |
|---|---|
| `not_found` | daemon returned 404 |
| `conflict` | daemon returned 409 (e.g. network still has endpoints) |
| `not_modified` | daemon returned 304 |
| `invalid` | rejected by our validation before reaching the daemon |
| `no_runtime` | nothing selected |
| `runtime_unavailable` | daemon unreachable |
| `engine`, `io`, `other` | everything else |

`AppError::message()` unwraps the daemon's own text rather than bollard's wrapper
prose, so the UI shows "network has active endpoints" and not a Rust type name.

The Electron backend returned HTTP 500 for every failure regardless of cause,
which is why the UI could never distinguish "no such container" from "daemon is
down". The integration test `maps_error_kinds` pins this, on both runtimes.

## Frontend

| File | Role |
|---|---|
| [`api.ts`](../tauri-rs/src/api.ts) | Typed `invoke()` wrappers and stream subscriptions. The only file that knows Tauri exists. |
| [`types.ts`](../tauri-rs/src/types.ts) | TS mirrors of `model.rs` |
| [`ui.tsx`](../tauri-rs/src/ui.tsx) | Shared primitives — Button, Modal, Table, Badge, toasts |
| [`hooks.ts`](../tauri-rs/src/hooks.ts) | `usePolled`, `useBusyMap`, `useDebounced`, `usePersisted` |
| [`views/`](../tauri-rs/src/views/) | One file per screen |

`usePolled` pauses while the window is hidden and catches up on
`visibilitychange`. It distinguishes *initial* load from *refresh* so a poll
never blanks a populated table. `useBusyMap` keys spinners per row-action, so
stopping one container doesn't grey out every other button.

Charts are hand-rolled SVG sparklines (~30 lines in `Containers.tsx`) rather than
a charting dependency, which keeps the bundle at ~250 kB / 75 kB gzipped.
