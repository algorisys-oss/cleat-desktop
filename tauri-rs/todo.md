# Cleat — Tauri + Rust

Modern, high-performance, multi-runtime container management app.

## Tech Stack

- **Backend**: Rust (Tauri v2 commands, async with Tokio, `bollard` Engine API client)
- **Frontend**: TypeScript + React 19 (Vite 7), Tailwind CSS v4
- **Container Runtimes**: Docker, Podman (nerdctl/containerd pending)
- **Desktop**: Tauri v2

## Running it

```
./dev-start.sh          # dev mode (from repo root)
./dev-start.sh --test   # Rust test suite
```

## Layout

```
src-tauri/src/
  model.rs            DTOs shared with the frontend (camelCase over IPC)
  error.rs            AppError + daemon status-code mapping
  state.rs            active runtime + live stream registry
  commands.rs         every #[tauri::command]
  runtime/
    mod.rs            ContainerRuntime trait, detection, connect()
    engine.rs         shared Docker Engine API client (both backends use it)
    docker.rs         Docker backend
    podman.rs         Podman backend (socket discovery + identity check)
    compose.rs        compose subprocess (argv only, never a shell)
src/
  api.ts              typed invoke() wrappers + stream subscriptions
  types.ts            TS mirrors of model.rs
  ui.tsx              shared primitives
  hooks.ts            usePolled / useBusyMap / usePersisted
  views/              Dashboard, Containers, Images, Networks, Volumes, Compose
```

---

## Phase 1 — Project Scaffold & Core

- [x] Initialize Tauri v2 project with React + TypeScript + Vite frontend
- [x] Set up Rust workspace structure (`src-tauri/`)
- [x] Configure Tailwind CSS v4
- [x] Define shared types (Rust structs + TS types) for containers, images, networks, volumes
- [x] Implement runtime detection — auto-detect Docker and Podman on the system
- [x] Create runtime abstraction trait in Rust (`ContainerRuntime`) so all backends share one interface
- [x] Implement Docker runtime backend (via Docker Engine API / bollard crate)
- [x] Implement Podman runtime backend (via Podman REST API)
- [x] Add runtime switcher in UI (select Docker / Podman)
- [ ] nerdctl / containerd backend

## Phase 2 — Container Management

- [x] List all containers (running, stopped, paused) with real-time status
- [x] Start / Stop / Restart / Pause / Unpause containers
- [x] Inspect container (full JSON detail view)
- [x] Live container logs with streaming (follow mode, search, stdout/stderr filter)
- [x] Real-time container stats (CPU, memory, network, block I/O) with sparkline charts
- [x] Container health check status
- [x] Remove container (with force and volume cleanup options)
- [x] Container services (list/start/stop/restart units via the container's init system)
- [x] Create container from image — backend command + validation
- [x] Create-container UI form — ports, env, volumes, network, restart policy,
      auto-remove, and quote-aware argv with a preview of the tokenisation
- [x] Exec into container (integrated terminal via PTY) — TTY exec with stdin,
      resize tracking, and a per-image shell probe; xterm.js front end
- [ ] Container file browser
- [ ] Health check *history*

## Phase 3 — Image Management

- [x] List all images with tags, size, creation date
- [x] Pull image with real-time per-layer progress bar
- [x] Remove / Force remove image
- [x] Image layer inspection (history)
- [x] Prune unused images
- [ ] Build image from Dockerfile (with build log streaming)
- [ ] Tag and push image to registry
- [ ] Image vulnerability scanning integration (Trivy / Grype)

## Phase 4 — Network Management

- [x] List all networks (driver, scope, subnets, connected containers)
- [x] Create network (bridge, macvlan, ipvlan, overlay; internal flag)
- [x] Inspect network (IPAM config, options)
- [x] Remove network
- [x] Connect / disconnect containers to/from networks — backend commands
- [ ] Connect/disconnect UI
- [ ] Network topology visualization (graph view)

## Phase 5 — Volume Management

- [x] List all volumes (name, driver, mount point, size)
- [x] Create volume
- [x] Inspect volume (labels, usage, creation date)
- [x] Remove volume
- [x] Prune unused volumes
- [ ] Volume usage stats (which containers use which volumes)

## Phase 6 — Compose / Stack Management

- [x] Parse and display compose services (`compose ps --format json`, both output shapes)
- [x] Start / Stop / Restart compose stacks
- [x] Per-service restart
- [x] Native directory picker for the project dir
- [x] Support Podman Compose (`podman compose` / `podman-compose` auto-detected)
- [ ] View per-service logs within a stack
- [ ] Scale services up/down
- [ ] Compose file editor with YAML validation

## Phase 7 — System & Dashboard

- [x] Dashboard with system overview (container counts by state, image count)
- [x] System info (runtime version, API version, OS, kernel, storage driver, host resources)
- [ ] System-wide resource usage graphs over time
- [x] Activity log — every runtime call with args, timing and outcome, via a
      decorator on `ContainerRuntime` so nothing can bypass it
- [ ] Event stream (real-time Docker/Podman events)
- [ ] Disk usage breakdown (images, containers, volumes, build cache)
- [ ] Prune all unused resources in one action

## Phase 8 — UX & Polish

- [x] Dark theme
- [x] Search/filter on every list view
- [x] Toast notifications for action results
- [x] Polling pauses while the window is hidden
- [ ] Light theme + system preference detection
- [ ] Keyboard shortcuts
- [ ] Global search across all resource types
- [ ] Configurable refresh intervals
- [ ] Settings page
- [ ] Tray icon with quick actions

## Phase 9 — Advanced Features

- [ ] Multi-host management (remote Docker/Podman daemons)
- [ ] Container resource limits editor
- [ ] Registry management (login, browse, private registries)
- [ ] Kubernetes pod/service viewer
- [ ] Export/import containers and images
- [ ] Container templates

## Phase 10 — Build & Distribution

- [x] Release binary builds (`npm run tauri build`)
- [x] Cross-platform bundles (.deb/.rpm/.AppImage, .dmg, .msi) — built by CI,
      one runner per platform; Tauri cannot cross-compile these locally
- [x] CI/CD pipeline for releases — `.github/workflows/release.yml`, triggered
      by a `v*` tag, which `/shipit` pushes after bumping the version
- [x] Version shown in the UI status bar, incremented on every release
- [ ] Auto-update mechanism (Tauri updater)
- [ ] Application signing — nothing is signed, so Gatekeeper and SmartScreen
      both object to released builds
- [ ] Verify the macOS and Windows bundles actually run (they compile in CI;
      nobody has launched them)

---

## Testing

`cd src-tauri && cargo test` — 69 tests.

- **27 unit**: port-spec parsing, service-name validation (injection cases),
  systemctl output parsing, CPU%/memory reduction, compose `ps` JSON parsing,
  exec base64 framing (control bytes, invalid UTF-8, split sequences), and the
  exec session registry.
- **18 Docker + 21 Podman integration**: both runtimes run the *same* shared
  suite (`tests/common/mod.rs`) — system summary, listing and DTO mapping,
  volume/network round-trips, container lifecycle, streaming stats and log
  follow, interactive exec (TTY round trip, resize, shell probe), error-kind
  mapping, injection rejection. Plus per-runtime specifics:
  compose front-end, Podman identity, overlay-driver rejection, and a
  Docker→Podman image copy.
- **3 compose streaming**: output arrives before the command completes,
  `down` rejects a service argument, injected service names are rejected.

All skip cleanly when a runtime is unreachable. Created resources are prefixed
`cleat-test-` and removed in the same test, including on failure paths.
