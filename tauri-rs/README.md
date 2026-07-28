# Cleat

Container management for Docker and Podman. Tauri v2 + Rust backend, React + TypeScript frontend.

This replaces a discontinued Electron implementation; see
[`../docs/migration.md`](../docs/migration.md).

Full documentation lives in [`../docs/`](../docs/):
[runtimes](../docs/runtimes.md) · [architecture](../docs/architecture.md) ·
[security](../docs/security.md) · [migration](../docs/migration.md) ·
[testing](../docs/testing.md)

## Requirements

- Rust (stable) — https://rustup.rs
- Node 20+
- Docker and/or Podman
- Linux: `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`

## Running

From the repository root:

```sh
./dev-start.sh          # dev mode with hot reload
./dev-start.sh --test   # Rust test suite
./dev-start.sh --clean  # wipe node_modules + target, then run
```

Or directly:

```sh
npm install
npm run tauri dev
npm run tauri build     # release bundles
```

## Architecture

The frontend never touches a container daemon. Every operation is a Tauri IPC
command into the Rust process, which owns the only connection to the runtime.

```
React view  ──invoke()──>  #[tauri::command]  ──>  dyn ContainerRuntime
                                                      ├── DockerRuntime  ┐
                                                      └── PodmanRuntime  ┴─> Engine (bollard)
```

`ContainerRuntime` (`src-tauri/src/runtime/mod.rs`) is the single interface every
command handler talks to. Docker and Podman both speak the Docker Engine API, so
they share `engine.rs`; the per-runtime files hold only what actually differs
(socket discovery, compose front-end, unsupported operations). Adding
nerdctl/containerd means implementing the trait — no handler changes.

### Streaming

Logs, stats, and pull progress are long-lived streams, not polls. A command
spawns a task that pumps the runtime stream onto a Tauri event channel and
registers it in `AppState` so it can be cancelled; the frontend's `subscribe*`
helpers in `src/api.ts` return a dispose function wired to React effect cleanup.

### Security notes

- No HTTP server, no listening port, no CORS. The Electron version bound
  `0.0.0.0:3000` with permissive CORS, which exposed root-equivalent Docker
  control to anything on the network.
- Subprocesses (compose only — it has no API) are built as argv vectors, never
  shell strings. Service names are validated against a character whitelist and
  actions against a fixed verb list before reaching `create_exec`.
- Compose project directories are canonicalised and checked before use.

## Testing

```sh
cd src-tauri && cargo test
```

Integration tests run against a live daemon and skip cleanly without one. They
create resources prefixed `cleat-test-` and remove them in the same test.
