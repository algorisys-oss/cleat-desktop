# CLAUDE.md

Working notes for Cleat — a Tauri v2 + React desktop app for managing Docker and
Podman containers. Read [README.md](README.md) for what it does and
[docs/](docs/) for how it is built.

## Layout

```
tauri-rs/               the app (name is scaffold residue, not meaningful)
  src/                  React + TypeScript frontend
  src-tauri/src/        Rust backend — commands.rs, state.rs, runtime/
scripts/bump-version.sh version bump across all five files
.github/workflows/      release CI
```

## Releasing — "shipit"

When the user says **shipit** (or "ship it", "cut a release"), run the
[`/shipit`](.claude/skills/shipit/SKILL.md) skill. In short: bump the version,
verify, tag, push; CI publishes executables to GitHub Releases.

Default bump is **patch**. `shipit minor` / `shipit major` / `shipit 1.2.3`
override it.

**Every shipit increments the version.** That is the point — the status bar
shows the running version, so it has to move for the display to mean anything.

## The version lives in five files

`scripts/bump-version.sh` is the only supported way to change it. Never edit
these by hand — Tauri names bundles from `tauri.conf.json`, cargo refuses to
build against a stale lock, and a release whose artifacts disagree with its tag
is worse than no release.

| File | Why |
|---|---|
| `tauri-rs/src-tauri/tauri.conf.json` | **Source of truth.** Names the bundles; the UI reads it |
| `tauri-rs/package.json` | npm metadata |
| `tauri-rs/package-lock.json` | must match package.json |
| `tauri-rs/src-tauri/Cargo.toml` | crate version |
| `tauri-rs/src-tauri/Cargo.lock` | regenerated, not edited |

The status bar version is injected at build time by a `define` in
`vite.config.ts` reading `tauri.conf.json`, exposed as `__APP_VERSION__` and
rendered by `StatusBar` in `src/App.tsx`. It cannot drift from the shipped
bundle, which is the reason for that indirection.

## Commands

```sh
./dev-start.sh              # dev mode
./dev-start.sh --test       # Rust test suite
cd tauri-rs && npx tsc --noEmit
cd tauri-rs/src-tauri && cargo test && cargo fmt && cargo clippy --all-targets
cd tauri-rs && npm run tauri build      # local Linux bundles only
```

Cross-platform bundles are **CI-only** — Tauri builds for its host platform, so
a Linux machine cannot produce a `.dmg` or `.msi`. Don't try.

## Conventions that matter here

**Never build a shell string.** Argv vectors only. Compose is the sole
subprocess because it has no API. See [docs/security.md](docs/security.md) —
this is the defect the Electron predecessor shipped.

The interactive terminal (`exec_start`) takes unconstrained argv on purpose and
is *not* a violation of the above; the reasoning is written down in
docs/security.md and should not be "fixed" without reading it.

**Docker and Podman run the same test suite.** Assertions live in
`tests/common/mod.rs` and both backends run them. A test that special-cases a
runtime proves nothing about the abstraction. Tests skip cleanly when a runtime
is unreachable, prefix resources `cleat-test-`, and clean up on failure paths.

**The tests run concurrently against a live daemon.** Anything comparing global
container state across two calls is racy — see the comment in
`suite::list_containers`.

**One-way streams vs exec.** Logs, stats, pulls and compose are one-way and
share the `openStream`/`register_stream` pattern. Exec is the only bidirectional
path and parks its stdin writer in `AppState`; both registries must be cleared
together. See [docs/architecture.md](docs/architecture.md).

## Gotchas

**Snap-confined terminals** (VS Code snap) export `GTK_PATH` into the snap; GTK
then loads modules that drag in the snap's older glibc and the app dies with
`undefined symbol: __libc_pthread_init`. `dev-start.sh` strips these. Note that
`$SNAP` and `LD_LIBRARY_PATH` can both be unset while this still happens —
`GTK_PATH` is the actual trigger.

**Native controls need `color-scheme: dark`.** WebKitGTK otherwise draws
`<select>` with the light system theme, giving unreadable text regardless of the
element's classes. The popup list is a platform widget and does not inherit the
select's Tailwind classes, so option colours are set separately in `styles.css`.

**`cargo fmt` reformats files beyond the ones you touched** — the repo is not
currently fmt-clean. Don't sweep unrelated churn into a feature commit.

**There is no `--version` flag.** The binary takes no arguments and opens the
window.

## Not claimed

Nothing is code-signed — macOS Gatekeeper and Windows SmartScreen will both
object to released builds. Only Linux has been exercised at runtime; the macOS
and Windows CI builds compile but nobody has run them.

**Windows compiled for the first time in v0.2.1.** Every release before that
advertised `.msi`/`.exe` artifacts that were never produced — the job failed in
`hyperlocal`, which is unix-sockets-only and sat in plain `[dependencies]`.
Transport crates now hang off `[target.'cfg(unix)']` / `[target.'cfg(windows)']`
and bollard carries its `pipe` feature there. Podman is compiled out on Windows
and reports why. Verify a Windows change with:

```sh
rustup target add x86_64-pc-windows-msvc
cargo check --target x86_64-pc-windows-msvc     # needs llvm-rc to get past tauri-winres
```
