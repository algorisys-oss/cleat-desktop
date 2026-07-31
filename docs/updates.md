# Updates

Cleat checks GitHub Releases for a newer version, downloads it in the
background, and offers it in the status bar. It never installs on its own.

## What can update itself, and what cannot

The updater replaces the application in place, so it only works where the
application *is* the thing that was installed.

| Artifact | Self-updates | Why |
|---|---|---|
| `.AppImage` | yes | one file, replaced in place |
| `.msi`, `.exe` (NSIS) | yes | the installer is re-run in passive mode |
| `.app` (macOS) | yes | the bundle is swapped, then the app relaunches |
| `.deb`, `.rpm` | **no** | owned by the system package manager; Cleat overwriting files under `/usr` behind `apt`'s back is not an improvement |
| bare `cleat` executable | **no** | owned by whoever put it there |

Installs in the second group are not left in the dark: the check still runs, and
the status bar links to the releases page instead of offering a button. Tauri's
updater is what enforces this — it refuses on anything it cannot replace safely,
and [`src/updates.ts`](../tauri-rs/src/updates.ts) turns that refusal into the
link rather than an error.

## How it decides

1. Eight seconds after launch (once the window has painted and the runtime is
   picked), the app fetches `latest.json` from the endpoint in
   `tauri.conf.json`, which points at the latest release's assets.
2. Tauri compares its own version against `version` in that file.
3. If it is newer, the platform's artifact is downloaded and **verified against
   the public key baked into the binary**. An update that does not verify is
   discarded — this is the reason the signing key below is not optional.
4. The status bar offers *Update to x.y.z*. Taking it installs and relaunches;
   dismissing it keeps the download and offers again next launch.

A failed check is silent unless the user asked for it by clicking the version.
No network, a proxy, or a firewall are all ordinary conditions and none of them
deserve a dialog on startup.

## The signing key

Update signing is **minisign, and has nothing to do with OS code signing**.
It proves an update came from this project; it does not stop macOS Gatekeeper or
Windows SmartScreen complaining on first install, which is still true and still
documented in the release notes.

The keypair lives outside the repository. The public half is committed in
`tauri.conf.json` (it is public by design — it is what the shipped binary uses
to verify), and the private half must never be:

```sh
# Generate once. Keep the file; back it up somewhere you would not lose.
cd tauri-rs
npx tauri signer generate -w ~/.tauri/cleat-updater.key
```

Then in the repository settings, under **Settings → Secrets and variables →
Actions**, add:

| Secret | Value |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | the contents of `~/.tauri/cleat-updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the password used above, or empty if none |

The release workflow checks for the first one before it builds anything, so a
missing secret fails in ten seconds with a message rather than twenty minutes in
on four runners.

**Losing the private key is not fatal but it is expensive.** Every already
installed copy verifies against the old public key, so a new keypair means
existing installs stop accepting updates and have to be reinstalled by hand.
Back it up.

## Building signed artifacts locally

```sh
cd tauri-rs
TAURI_SIGNING_PRIVATE_KEY="$HOME/.tauri/cleat-updater.key" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" \
  npm run tauri build
```

Each updater artifact lands next to its bundle with a `.sig` beside it. Without
the environment variables the build fails, because `createUpdaterArtifacts` is
on in `tauri.conf.json` — that is deliberate, since a release that quietly
shipped unsigned artifacts would leave every installed copy unable to update.

## Releasing

Nothing extra. `/shipit` bumps the version, tags, and pushes; CI builds, signs,
and `tauri-action` writes `latest.json` alongside the artifacts. The endpoint is
`releases/latest/download/latest.json`, so it always resolves to the newest
non-prerelease release — which is why the workflow publishes with
`prerelease: false`. A release marked as a prerelease will not be offered to
anyone, which is a useful way to stage one.
