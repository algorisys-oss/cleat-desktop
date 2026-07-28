# Container runtimes

## Supported today

| Runtime | Status | How we reach it |
|---|---|---|
| **Docker Engine** (open source / Moby) | Supported, verified against a live daemon | Docker Engine API over `/var/run/docker.sock`, or `DOCKER_HOST` |
| **Podman** | Supported, verified against a live daemon (4.9.3) | Podman's Docker-compatible REST API over its own socket |
| nerdctl / containerd | Not implemented | — |

Both runtimes are held to the **same integration suite** — see
[testing.md](testing.md). That is the only meaningful demonstration that the
abstraction holds: identical assertions, two backends, no per-runtime
special-casing in the tests.

### Docker Engine

This is the open-source engine — the Moby project — not Docker Desktop. There is
no dependency on Docker Desktop, its licence, or its VM. We speak the Engine API
directly through the [`bollard`](https://crates.io/crates/bollard) crate.

Connection is `Docker::connect_with_defaults()` in
[`runtime/docker.rs`](../tauri-rs/src-tauri/src/runtime/docker.rs), which resolves in this order:

1. `DOCKER_HOST` if set (unix socket, TCP, or SSH)
2. the platform default — `/var/run/docker.sock` on Linux/macOS, the named pipe on Windows

A consequence worth knowing: **anything that exposes an Engine-API socket already
works**, with no code change, by pointing `DOCKER_HOST` at it. That covers Colima,
Rancher Desktop, OrbStack, a rootless Docker socket, and remote daemons. Those
appear in the UI as "Docker" because that is the API they serve.

The reference daemon used during development reported:

```
Docker Engine - Community 29.6.1
components: Engine, containerd, runc, docker-init
API version 1.55
```

### Podman

Podman exposes a Docker-compatible REST API, so it reuses the same Engine client.
[`runtime/podman.rs`](../tauri-rs/src-tauri/src/runtime/podman.rs) holds only what genuinely differs:

- **Socket discovery** — `CONTAINER_HOST`, then rootless
  `$XDG_RUNTIME_DIR/podman/podman.sock`, then rootful `/run/podman/podman.sock`.
- **Identity check** — after connecting, `/version` must identify as Podman.
- **Compose** — `podman compose` (Podman 4.7+) or standalone `podman-compose`,
  detected at connect time so a missing binary is reported when you select the
  runtime rather than on your first compose action.
- **Unsupported operations** — `overlay` network driver is rejected with a reason
  (netavark has no such driver); rootless `pause` failures are annotated to
  explain the cgroups v2 requirement instead of surfacing a bare daemon error.

Enable the API socket with:

```sh
systemctl --user start podman.socket     # rootless
sudo systemctl start podman.socket       # rootful
```

#### Behavioural differences found by running the shared suite

Podman is Docker-compatible, not Docker-identical. What the suite actually
caught, and how it is handled:

| Difference | Handling |
|---|---|
| **Health status.** Docker omits `State.Health` when a container has no healthcheck; Podman includes it with an empty status. | `normalize_health()` collapses both to `"none"`. Without it, Podman containers rendered an empty health badge, because the UI keys off `health !== "none"`. |
| **Overlay driver.** netavark has no overlay driver. | Rejected up front with a reason naming the driver. |
| **Rootless pause.** Fails on cgroups v1. | Error annotated to explain the cgroups v2 requirement. |
| **Compose front-end.** `podman compose` delegates to whatever provider is installed; on some systems that is Docker's own compose plugin. | Detected at connect time; whichever provider is present is used. |

> **Verification status.** Verified against Podman 4.9.3 (rootless, cgroups v2)
> on Ubuntu 24.04 — 16 integration tests including the container lifecycle,
> streaming stats, and log follow. Not yet exercised: rootful Podman, cgroups
> v1, `podman-compose` as the provider, or Podman on macOS (where it runs in a
> VM).

#### Footgun: bollard's Podman defaults fall back to Docker

`Docker::connect_with_podman_defaults()` ends with:

```rust
// Fall back to default Docker socket
Docker::connect_with_unix(DEFAULT_SOCKET, DEFAULT_TIMEOUT, API_DEFAULT_VERSION)
```

On a Docker-only machine that made Podman report itself **available, with
Docker's version number**, and selecting it would have served Docker's containers
under the Podman label. This was caught by looking at the runtime switcher in the
running app, not by a test.

We do not use that helper. `podman_socket_path()` performs its own discovery and
returns `RuntimeUnavailable` when no Podman socket exists, and `is_podman()`
confirms the daemon's identity from the `components` array of `/version` before
the runtime is accepted. The regression test is
`podman_does_not_masquerade_as_docker` in
[`tests/live_docker.rs`](../tauri-rs/src-tauri/tests/live_docker.rs).

## Moving images between runtimes

Docker and Podman keep **entirely separate stores**. Switching runtimes shows a
different set of images, containers, volumes, and networks — nothing is shared,
and that is inherent to the runtimes, not a limitation of this app. Rootless and
rootful Podman are separate from each other too.

Images can be transferred, and the app does this natively: the Images view has a
per-image **→ podman** / **→ docker** action. It streams the source's
`GET /images/{name}/get` straight into the destination's `POST /images/load` —
no temp file, no CLI, and no shell. The image remains in the source runtime.

Equivalent by hand:

```sh
docker save myimage:tag | podman load
podman save myimage:tag | docker load
podman pull docker-daemon:myimage:tag   # direct, needs docker socket access
```

**Only images transfer.** Containers are daemon-specific state — recreate them
on the target runtime from the image. Volumes must be copied separately.

## How detection works

`detect_runtimes()` probes every known runtime concurrently and returns a
`RuntimeInfo` for each — including the unavailable ones, with the reason attached:

```rust
pub struct RuntimeInfo {
    pub kind: RuntimeKind,
    pub available: bool,          // the daemon answered
    pub installed: bool,          // present on this machine at all
    pub version: Option<String>,
    pub api_version: Option<String>,
    pub detail: Option<String>,   // why it isn't available
}
```

Each probe genuinely pings the daemon. A runtime whose binary is installed but
whose socket is dead is reported unavailable rather than appearing selectable and
then failing on first use.

### `installed` vs `available`, and what the UI shows

These are two different questions and conflating them produces a bad UI either
way.

| `installed` | `available` | Meaning | UI |
|---|---|---|---|
| ✅ | ✅ | Working | Listed, selectable, version shown |
| ✅ | ❌ | **Installed but not running** | Listed, greyed out, **with the fix attached** |
| ❌ | ❌ | Not on this machine | **Hidden** |

The middle row is the one that matters. "Podman is installed but its socket
isn't running" is a state the user can fix in one command, and hiding it is
actively unhelpful — they have Podman, they expect to see it, and silence gives
them nothing to act on. So it stays visible with
`systemctl --user start podman.socket` in the tooltip.

The bottom row is noise. Showing "Podman — unavailable" to someone who has never
installed Podman is clutter about a product they don't use.

`installed` is determined without touching the daemon: CLI binary on `PATH`, a
socket at a known path, or an explicit `DOCKER_HOST`/`CONTAINER_HOST`. A remote
daemon reached via `DOCKER_HOST` counts as installed even with no local binary.

Two deliberate choices:

- **The backend always returns every runtime**, including uninstalled ones.
  Filtering is the UI's decision. A caller that wants the full picture —
  diagnostics, a "show all runtimes" toggle — shouldn't have to re-probe, and
  `detect_runtimes()` shouldn't have to guess who's asking.
- **The active runtime is never hidden**, whatever the flags say. The UI can't
  end up hiding the thing it is currently driving.

The switcher shows an unobtrusive "*n* not installed" line when anything is
filtered, so the hiding is discoverable rather than a mystery.

On startup the backend calls `auto_select()`, which tries Docker then Podman and
emits a `runtime:ready` event with the result, so the window paints immediately
instead of blocking on probes.

## Adding a runtime

The amount of work splits sharply depending on the API the runtime speaks.

### Case A — it speaks the Docker Engine API

**No code.** Set `DOCKER_HOST` and it works as "Docker".

If you want it as its own entry in the switcher (distinct icon, version, and
socket discovery), it is roughly 80 lines — copy the shape of `podman.rs`, reuse
`Engine`, override only discovery and any unsupported operations.

### Case B — it does not (containerd/CRI, Kubernetes, a cloud API)

Implement the trait. Four edits:

**1. New backend file** — `src-tauri/src/runtime/nerdctl.rs`:

```rust
pub struct NerdctlRuntime { /* your client */ }

#[async_trait]
impl ContainerRuntime for NerdctlRuntime {
    fn kind(&self) -> RuntimeKind { RuntimeKind::Nerdctl }
    async fn list_containers(&self, all: bool) -> AppResult<Vec<Container>> { ... }
    // ...40 methods total
}

pub async fn probe() -> RuntimeInfo { ... }
```

40 methods sounds heavy, but most are thin. The real work is mapping the
runtime's model onto the DTOs in [`model.rs`](../tauri-rs/src-tauri/src/model.rs) —
that is the contract, and it is deliberately flatter than any one runtime's
native schema.

**2. `model.rs`** — add the `RuntimeKind` variant.

**3. [`runtime/mod.rs`](../tauri-rs/src-tauri/src/runtime/mod.rs)** — register it:

```rust
pub async fn detect_runtimes() -> Vec<RuntimeInfo> {
    let (docker, podman, nerdctl) =
        tokio::join!(docker::probe(), podman::probe(), nerdctl::probe());
    vec![docker, podman, nerdctl]
}

pub async fn connect(kind: RuntimeKind) -> AppResult<Box<dyn ContainerRuntime>> {
    match kind {
        RuntimeKind::Docker  => Ok(Box::new(docker::DockerRuntime::connect().await?)),
        RuntimeKind::Podman  => Ok(Box::new(podman::PodmanRuntime::connect().await?)),
        RuntimeKind::Nerdctl => Ok(Box::new(nerdctl::NerdctlRuntime::connect().await?)),
    }
}
```

`connect()` is **the only exhaustive match on `RuntimeKind` in the whole
backend**, so adding a variant produces exactly one compiler error pointing
straight at it.

`detect_runtimes()` is a plain list, not a match — **the compiler will not remind
you to add your probe there.** Forget it and your runtime works when selected but
never appears in the switcher. It is the one step with no safety net.

**4. [`types.ts`](../tauri-rs/src/types.ts)** — widen the union:

```ts
export type RuntimeKind = "docker" | "podman" | "nerdctl";
```

### What you do *not* touch

All **46 Tauri commands** go through `state.runtime().await?`, which hands back
`Box<dyn ContainerRuntime>`. None of them names a concrete runtime. Neither does
any React component — the frontend only ever sees the DTOs.

That is the whole reason the trait exists rather than calling `bollard` directly
from the command handlers: adding a runtime is additive, not a refactor.

### Design notes for implementers

- **`compose_argv()` is on the trait, not assumed.** Compose has no API — it is a
  CLI tool — so every runtime brings its own front-end (`docker compose`,
  `podman compose`, `nerdctl compose`). This stays a subprocess call and must be
  built as argv; see [security.md](security.md).
- **Streams are `Pin<Box<dyn Stream>>`.** `follow_logs`, `stream_stats`, and
  `pull_image` return boxed streams so the trait stays object-safe. If your
  runtime has no native streaming, poll internally and produce a stream.
- **Fail with a reason.** When an operation is unsupported, return
  `AppError::Invalid` or `AppError::Conflict` with a sentence the user can act
  on. `podman.rs` does this for the overlay driver and rootless pause.
- **Don't fall back to another runtime's socket.** See the bollard footgun above.
