---
name: shipit
description: Cut a Cleat release — bump the version, verify, tag, push, and publish executables to GitHub Releases. Use when the user says "shipit", "ship it", "cut a release", "release Cleat", or invokes /shipit.
---

# shipit

Release Cleat. Bumps the version, verifies the build, tags, pushes, and lets CI
publish executables for Linux, macOS and Windows to GitHub Releases.

Default bump is **patch**. `/shipit minor`, `/shipit major`, or `/shipit 1.2.3`
override it.

## Why it works this way

Tauri bundles for its host platform only, so macOS and Windows executables
cannot be produced on the maintainer's Linux box. The tag push is the trigger;
[`.github/workflows/release.yml`](../../../.github/workflows/release.yml) fans
out across four runners and uploads everything to one release. Do not try to
build the other platforms locally — it cannot work.

## Steps

Run these in order. Stop and report if any step fails; do not push a half-done
release.

### 1. Refuse to ship a dirty or stale tree

```sh
git status --porcelain          # must be empty
git rev-parse --abbrev-ref HEAD # expect main
git fetch origin && git rev-list --left-right --count HEAD...origin/main
```

- Uncommitted changes → stop and ask. Never `git add -A` on the user's behalf here.
- Not on `main` → stop and ask; releasing from a branch tags the wrong commit.
- Behind `origin/main` → stop and ask; the release would omit pushed work.

### 2. Verify before bumping

Cheaper to fail now than to unwind a tag.

```sh
cd tauri-rs && npm ci && npx tsc --noEmit
cd src-tauri && cargo test
```

All tests must pass. The integration tests skip cleanly when no container
runtime is reachable — that is fine, but say so in the summary if it happens,
because a run that skipped them proves much less.

### 3. Bump

```sh
NEW="$(scripts/bump-version.sh ${ARG:-patch})"    # prints e.g. 0.1.1
```

The script updates all five files that record the version and verifies they
agree; it exits non-zero on a partial bump. Never hand-edit these — see
[CLAUDE.md](../../../CLAUDE.md).

### 4. Confirm the tag is free

```sh
git tag -l "v$NEW"
git ls-remote --tags origin "refs/tags/v$NEW"
```

Either being non-empty means that version already shipped. Stop and ask —
re-tagging a released version breaks anyone who already downloaded it.

### 5. Commit, tag, push

```sh
git add -A
git commit -m "Release v$NEW"
git tag -a "v$NEW" -m "Cleat v$NEW"
git push origin main
git push origin "v$NEW"
```

Push the branch before the tag. If the tag lands first, CI builds a commit that
is not yet on `main`.

### 6. Report where it is

```sh
gh run list --workflow=release.yml --limit 1     # needs gh auth
```

`gh` may not be authenticated. Do not attempt to log in or ask for a token —
just give the user the links:

- Progress: `https://github.com/algorisys-oss/cleat-desktop/actions`
- Release: `https://github.com/algorisys-oss/cleat-desktop/releases/tag/v$NEW`

The build takes roughly 10–20 minutes across four runners. Tell the user the
release appears only once CI finishes; the tag existing does not mean the
artifacts are up yet.

## What lands on the release

| Platform | Artifacts |
|---|---|
| Linux | `.deb`, `.rpm`, `.AppImage` |
| macOS | `.dmg` for Apple silicon and Intel |
| Windows | `.msi`, `.exe` |

## Say this in the summary

- The version, and that the status bar now shows it.
- That CI is still running and the release is not live until it finishes.
- **That nothing is code-signed** — macOS Gatekeeper and Windows SmartScreen
  will both object. This is a real limitation for anyone you send the link to,
  so do not omit it.

## If it goes wrong

A tag pushed by mistake, *before* anyone has downloaded it:

```sh
git push --delete origin "v$NEW"
git tag -d "v$NEW"
gh release delete "v$NEW" --yes    # if the release was created
```

Then revert the release commit. If the release has been public for any length
of time, do not delete it — ship a new patch version instead.
