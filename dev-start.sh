#!/usr/bin/env bash
# Start the Tauri app (tauri-rs/) in development mode.
#
# Usage:
#   ./dev-start.sh            # start the dev server + Tauri window
#   ./dev-start.sh --clean    # wipe node_modules + cargo target first
#   ./dev-start.sh --test     # run the Rust test suite instead of launching
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/tauri-rs"

info()  { printf '\033[36m==>\033[0m %s\n' "$*"; }
warn()  { printf '\033[33mwarn:\033[0m %s\n' "$*" >&2; }
die()   { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || die "$1 not found on PATH. $2"
}

# --- prerequisites -----------------------------------------------------------
need node  "Install Node 20+ (https://nodejs.org or nvm)."
need npm   "Ships with Node."
need cargo "Install the Rust toolchain (https://rustup.rs)."

node_major="$(node -p 'process.versions.node.split(".")[0]')"
[ "$node_major" -ge 20 ] || die "Node 20+ required, found $(node -v)."

command -v docker >/dev/null 2>&1 || warn "docker not on PATH — the app will start but find no runtime."

# Linux WebKit deps: tauri dev fails late and cryptically without them.
if [ "$(uname -s)" = "Linux" ] && command -v pkg-config >/dev/null 2>&1; then
  pkg-config --exists webkit2gtk-4.1 || warn \
    "webkit2gtk-4.1 dev files not found. On Debian/Ubuntu:
       sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \\
         libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev"
fi

# --- scaffold check ----------------------------------------------------------
if [ ! -f "$APP_DIR/package.json" ]; then
  die "No Tauri project in $APP_DIR.

Scaffold it once with:
  cd $(dirname "$APP_DIR")
  npm create tauri-app@latest tauri-rs -- --template react-ts --manager npm

Then re-run this script."
fi

# Snap-confined terminals (e.g. the VS Code snap) export a library path into
# /snap/core*/lib. The Tauri binary then loads the snap's older glibc and dies
# with "undefined symbol: __libc_pthread_init, version GLIBC_PRIVATE". Drop
# those before launching.
#
# Checked two ways because either alone has a hole: $SNAP identifies the common
# case, but a terminal can scrub it while still exporting snap loader paths, and
# that fails identically with the guard silently doing nothing.
snap_reason=""
case ":${LD_LIBRARY_PATH:-}:${LD_PRELOAD:-}:" in
  *:/snap/*) snap_reason="snap paths in LD_LIBRARY_PATH/LD_PRELOAD" ;;
esac
if [ -z "$snap_reason" ] && [ -n "${SNAP:-}" ]; then
  snap_reason="\$SNAP=$SNAP"
fi

if [ -n "$snap_reason" ]; then
  warn "snap environment detected ($snap_reason); clearing its library paths"
  unset LD_LIBRARY_PATH LD_PRELOAD GTK_PATH GTK_EXE_PREFIX GIO_MODULE_DIR
  unset GDK_PIXBUF_MODULE_FILE GSETTINGS_SCHEMA_DIR LOCPATH
fi

cd "$APP_DIR"

# --- optional clean ----------------------------------------------------------
if [ "${1:-}" = "--clean" ]; then
  info "Cleaning node_modules and src-tauri/target"
  rm -rf node_modules src-tauri/target
fi

# --- deps --------------------------------------------------------------------
if [ ! -d node_modules ]; then
  info "Installing npm dependencies"
  if [ -f package-lock.json ]; then npm ci; else npm install; fi
fi

# --- tests -------------------------------------------------------------------
# The integration tests need a reachable daemon; they skip cleanly without one.
if [ "${1:-}" = "--test" ]; then
  info "Running Rust test suite"
  cd src-tauri
  exec cargo test
fi

# --- run ---------------------------------------------------------------------
# First run compiles the whole Rust dependency tree — expect several minutes.
info "Starting Tauri dev (Ctrl+C to stop)"
if npm run | grep -qE '^  tauri$'; then
  exec npm run tauri dev
else
  need npx "Ships with npm."
  exec npx tauri dev
fi
