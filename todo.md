# Cleat — status

The app lives in [`tauri-rs/`](tauri-rs/) — Tauri v2 + Rust backend, React +
TypeScript frontend. Run it with `./dev-start.sh` from this directory.

- **Roadmap and per-phase status**: [tauri-rs/todo.md](tauri-rs/todo.md)
- **Why the architecture looks like this**: [docs/migration.md](docs/migration.md)
- **Everything else**: [docs/](docs/)

## Next up

Backend command exists and is tested; only the UI is missing:

- [x] **Create container from image** — form wired to `create_container`,
      reachable from Run on any image row and New container in the list.
- [ ] **Connect / disconnect container to network** — `connect_network` and
      `disconnect_network` exist. Needs a UI affordance.

Not started:

- [ ] Compose per-service logs
- [ ] Container file browser
- [ ] Image build from Dockerfile
- [ ] Event stream, disk-usage breakdown
- [ ] nerdctl / containerd backend
- [ ] Light theme, keyboard shortcuts, settings page
- [ ] Cross-platform bundles + signing

## Housekeeping

- [x] MIT `LICENSE` at the repo root, declared in `Cargo.toml`, `package.json`
      and the Tauri bundle config so the deb/rpm/msi carry it too
- [ ] Consider renaming `tauri-rs/` to something less scaffold-flavoured
- [ ] Verify on macOS and Windows (only Linux has been exercised)
