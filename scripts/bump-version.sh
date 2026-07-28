#!/usr/bin/env bash
# Bump the app version everywhere it is recorded.
#
# The version lives in five files and they must not drift: Tauri names the
# bundles from tauri.conf.json, cargo refuses to build with a stale lock, and
# the UI status bar reads tauri.conf.json through a Vite define. A release with
# mismatched numbers is worse than no release, so this is one script rather than
# five manual edits.
#
# Usage:
#   scripts/bump-version.sh                # patch: 0.1.0 -> 0.1.1
#   scripts/bump-version.sh minor          # 0.1.0 -> 0.2.0
#   scripts/bump-version.sh major          # 0.1.0 -> 1.0.0
#   scripts/bump-version.sh 1.2.3          # explicit
#   scripts/bump-version.sh patch --dry-run
#
# Prints the new version to stdout and nothing else, so callers can capture it:
#   NEW="$(scripts/bump-version.sh patch)"
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$ROOT/tauri-rs"
CONF="$APP/src-tauri/tauri.conf.json"
CARGO="$APP/src-tauri/Cargo.toml"
PKG="$APP/package.json"

# Diagnostics go to stderr; stdout carries only the new version.
info() { printf '\033[36m==>\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v jq >/dev/null 2>&1 || die "jq is required (sudo apt install jq)"

bump="patch"
dry_run=0
for arg in "$@"; do
  case "$arg" in
    major|minor|patch) bump="$arg" ;;
    --dry-run)         dry_run=1 ;;
    [0-9]*.[0-9]*.[0-9]*) bump="$arg" ;;
    *) die "unrecognised argument '$arg' (expected major|minor|patch|X.Y.Z|--dry-run)" ;;
  esac
done

[ -f "$CONF" ] || die "not found: $CONF"

current="$(jq -r '.version' "$CONF")"
[[ "$current" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "current version in tauri.conf.json is not X.Y.Z: '$current'"

case "$bump" in
  major|minor|patch)
    IFS=. read -r major minor patch <<<"$current"
    case "$bump" in
      major) major=$((major + 1)); minor=0; patch=0 ;;
      minor) minor=$((minor + 1)); patch=0 ;;
      patch) patch=$((patch + 1)) ;;
    esac
    next="$major.$minor.$patch"
    ;;
  *)
    next="$bump"
    [[ "$next" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "explicit version must be X.Y.Z, got '$next'"
    ;;
esac

[ "$next" != "$current" ] || die "version is already $next"

if [ "$dry_run" -eq 1 ]; then
  info "would bump $current -> $next (dry run, nothing written)"
  printf '%s\n' "$next"
  exit 0
fi

info "bumping $current -> $next"

# 1. tauri.conf.json — the source of truth. jq rather than sed: it round-trips
#    the JSON properly instead of pattern-matching a line that also appears in
#    the bundle config.
tmp="$(mktemp)"
jq --arg v "$next" '.version = $v' "$CONF" >"$tmp"
mv "$tmp" "$CONF"

# 2 + 3. package.json and package-lock.json, in one step so they cannot diverge.
( cd "$APP" && npm version "$next" --no-git-tag-version --allow-same-version >/dev/null )

# 4. Cargo.toml — only the version in [package], which is the first one in the
#    file. Bounded to that line so a dependency pinned to the same string is not
#    rewritten too.
python3 - "$CARGO" "$next" <<'PY'
import re, sys
path, new = sys.argv[1], sys.argv[2]
src = open(path).read()
out, n = re.subn(
    r'(?ms)(^\[package\]\n(?:(?!^\[).*?\n)?^version\s*=\s*")[^"]+(")',
    lambda m: m.group(1) + new + m.group(2),
    src, count=1)
if n != 1:
    sys.exit(f"failed to rewrite [package] version in {path}")
open(path, "w").write(out)
PY

# 5. Cargo.lock — regenerated rather than hand-edited. `cargo metadata` is the
#    cheapest command that syncs the lock without compiling anything.
( cd "$APP/src-tauri" && cargo metadata --format-version 1 --quiet >/dev/null )

# Verify every file agrees before reporting success. A silent partial bump would
# surface much later as a release whose artifacts disagree with its tag.
conf_v="$(jq -r '.version' "$CONF")"
pkg_v="$(jq -r '.version' "$PKG")"
lock_v="$(jq -r '.version' "$APP/package-lock.json")"
cargo_v="$(sed -n '/^\[package\]/,/^\[/p' "$CARGO" | sed -n 's/^version *= *"\(.*\)"/\1/p' | head -1)"
cargolock_v="$(grep -A1 '^name = "cleat"$' "$APP/src-tauri/Cargo.lock" | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"

for pair in "tauri.conf.json:$conf_v" "package.json:$pkg_v" "package-lock.json:$lock_v" \
            "Cargo.toml:$cargo_v" "Cargo.lock:$cargolock_v"; do
  name="${pair%%:*}"; value="${pair#*:}"
  [ "$value" = "$next" ] || die "$name is '$value', expected '$next' — version bump was partial"
done

info "all five version files now read $next"
printf '%s\n' "$next"
