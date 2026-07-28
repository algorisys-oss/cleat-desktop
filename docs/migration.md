# Migration: Electron → Tauri + Rust

Cleat supersedes an earlier Electron implementation. That code is discontinued
and is **not** part of this repository — it remains in the
[previous repository](https://github.com/rajeshpillai/docker-desktop-lite) for reference only.

This document is kept because the *reasons* for the rewrite, and the defects it
fixed, are still the best explanation of why the current architecture looks the
way it does.

## Why

Three reasons, in order of weight:

1. **The Express server had to go.** It bound `0.0.0.0:3000` with permissive CORS
   in front of a full Docker client, and it interpolated request data into shell
   commands. See [security.md](security.md).
2. **The interesting features needed streaming.** Log follow, live stats, and
   pull progress are all long-lived streams. The Express/fetch shape made them
   awkward enough that they were never built — logs were a one-shot `tail 100`,
   stats a single sample, and pull progress was `console.log`'d server-side where
   the UI could never see it.
3. **Multi-runtime support needed an abstraction.** Podman and nerdctl were on
   the roadmap; `dockerode` calls scattered across route handlers had no seam to
   add them at.

## What changed

| Area | Electron | Tauri + Rust |
|---|---|---|
| Transport | Express on `0.0.0.0:3000`, renderer `fetch()` | Tauri IPC — no port, no CORS |
| Docker client | `dockerode` | `bollard` |
| Runtimes | Docker only | Docker + Podman, both verified against live daemons |
| Compose | `exec()` shell string, `docker-compose` (v1, EOL) | argv subprocess, `docker compose` |
| In-container services | `docker exec … systemctl <user string>` (injectable, and the list route was broken) | `create_exec` argv + validated unit name |
| Logs | one-shot `tail 100` | streaming follow, search, stdout/stderr filter, 5k cap |
| Stats | single sample | 1 Hz stream, sparklines, cache-corrected memory |
| Pull progress | server-side `console.log` only | per-layer progress in the UI |
| Errors | HTTP 500 for everything | typed kinds (`not_found`, `conflict`, …) |
| Networks | name, driver, scope | + subnets and attached containers |
| Polling | bare `setInterval`, ran while hidden | pauses when the window is hidden |
| Tests | none | 45, across both runtimes |

## Feature parity checklist

Everything the Electron app did:

- [x] List containers (running + stopped) with name, ID, status
- [x] Start / stop containers
- [x] Inspect container
- [x] Container logs
- [x] Container statistics (CPU, memory, network I/O)
- [x] Container health check status
- [x] Container services (list, start, stop) — now actually works
- [x] Remove container
- [x] List / pull / remove images
- [x] List / create / remove networks
- [x] List / create / remove / inspect / prune volumes
- [x] Compose: list services, up, down
- [x] Sidebar navigation, modals, section switching

Added in the port:

- [x] Restart / pause / unpause containers
- [x] Streaming log follow with search and stream filter
- [x] Live stats with charts
- [x] Per-layer pull progress
- [x] Image layer history, image prune
- [x] Network inspect, internal networks, attached-container resolution
- [x] Compose: per-service restart, native directory picker, both `ps --format
      json` output shapes
- [x] Dashboard with system overview
- [x] Runtime switcher (Docker / Podman)
- [x] Search/filter on every list view
- [x] Toast notifications

## Outstanding

Backend command exists and is tested; only the UI is missing:

- [ ] **Create container from image** — `create_container` handles ports, env,
      volumes, network, restart policy, auto-remove. Needs a form.
- [ ] **Connect / disconnect container to network** — `connect_network` and
      `disconnect_network` exist. Needs UI affordance.

Not started:

- [ ] Exec into container (PTY terminal)
- [ ] Compose per-service logs
- [ ] Container file browser
- [ ] Image build from Dockerfile
- [ ] Event stream, disk-usage breakdown

The full roadmap is in [`tauri-rs/todo.md`](../tauri-rs/todo.md).

## Mapping old routes to commands

Useful when reading the old code:

| Electron route | Tauri command |
|---|---|
| `GET /containers` | `list_containers` |
| `GET /containers/:id/inspect` | `inspect_container` |
| `GET /containers/:id/logs` | `container_logs` / `follow_logs` |
| `GET /containers/:id/stats` | `container_stats` / `stream_stats` |
| `GET /containers/:id/health` | `container_health` |
| `POST /containers/:id/start` \| `/stop` | `start_container` / `stop_container` |
| `DELETE /containers/:id/remove` | `remove_container` |
| `GET /containers/:id/services` | `list_container_services` |
| `POST /containers/:id/services/:svc/start` \| `/stop` | `control_container_service` |
| `GET /images` | `list_images` |
| `POST /images/pull` | `pull_image` (streaming) |
| `DELETE /images/:id` | `remove_image` |
| `GET /networks` | `list_networks` |
| `POST /networks/create` | `create_network` |
| `DELETE /networks/:id` | `remove_network` |
| `GET /volumes` | `list_volumes` |
| `POST /volumes/create` | `create_volume` |
| `DELETE /volumes/:name` | `remove_volume` |
| `GET /volumes/:name/inspect` | `inspect_volume` |
| `DELETE /volumes/prune` | `prune_volumes` |
| `POST /compose/up` \| `/down` \| `/services` | `compose_up` / `compose_down` / `compose_services` |

Note the two duplicate `GET /containers/:id/inspect` registrations in the old
`app.js` — the second was dead code.
