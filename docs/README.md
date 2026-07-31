# Documentation

Reference for **Cleat** ([`tauri-rs/`](../tauri-rs/)) — the Tauri v2 +
Rust application that replaced an earlier Electron implementation, which is
no longer maintained and lives only in the [previous repository](https://github.com/rajeshpillai/docker-desktop-lite).

| Document | What it covers |
|---|---|
| [runtimes.md](runtimes.md) | Which container runtimes are supported, how detection works, and how to add another |
| [architecture.md](architecture.md) | Process model, the `ContainerRuntime` trait, streaming, state, error mapping |
| [security.md](security.md) | Threat model, the injection and network-exposure defects fixed in the port, and the rules that keep them fixed |
| [migration.md](migration.md) | Electron → Tauri feature mapping, what changed, what is still outstanding |
| [testing.md](testing.md) | Test layout, what the live-daemon tests do to your machine, how to run them |
| [updates.md](updates.md) | How in-app updates work, which install formats can use them, and the signing key the release needs |

## Quick orientation

```
docker-desktop-lite/
├── dev-start.sh          launch / test the app
├── docs/                 you are here
├── tauri-rs/             the current app
│   ├── src/              React + TypeScript frontend
│   └── src-tauri/        Rust backend
```

Run it from the repository root:

```sh
./dev-start.sh          # dev mode with hot reload
./dev-start.sh --test   # Rust test suite
./dev-start.sh --clean  # wipe node_modules + target, then run
```

## Size

| | Lines |
|---|---|
| Rust backend (`src-tauri/src`) | ~2,970 |
| Frontend (`src`) | ~3,540 |
| Tauri commands | 50 |
| `ContainerRuntime` trait methods | 40 |
| Tests | 69 (27 unit, 18 Docker, 21 Podman, 3 compose) |
