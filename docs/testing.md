# Testing

```sh
./dev-start.sh --test            # from the repository root
cd tauri-rs/src-tauri && cargo test
```

69 tests: 27 unit, 18 Docker integration, 21 Podman integration, 3 compose streaming.

The integration assertions live in `tests/common/mod.rs` and are **shared** —
`live_docker.rs` and `live_podman.rs` run the same `suite::*` functions against
their respective backends. Identical expectations against two runtimes is the
only real demonstration that the `ContainerRuntime` abstraction holds; a suite
that special-cased per runtime would prove nothing.

## Unit tests

Colocated with the code they cover. These target the logic where a bug is quiet
rather than loud — parsers, validators, and arithmetic.

**[`runtime/engine.rs`](../tauri-rs/src-tauri/src/runtime/engine.rs)**

| Test | Why it exists |
|---|---|
| `parses_plain_port_spec`, `parses_udp_port_spec`, `rejects_malformed_port_specs` | `"8080:80"` → `("8080", "80/tcp")`; non-numeric and missing-colon forms must fail |
| `accepts_ordinary_service_names` | `nginx`, `nginx.service`, `getty@tty1.service` must still work |
| `rejects_shell_metacharacters_in_service_names` | Eight injection payloads — `;`, `&&`, `\|`, `$(…)`, backticks, newline, `>`, leading `-`. See [security.md](security.md) |
| `parses_systemctl_units_output` | Column parsing for `list-units` |
| `skips_bullet_column_for_degraded_units` | systemd prefixes failed units with `●`, shifting every column |
| `ignores_non_service_lines` | Headers and blank lines must not become fake units |
| `computes_cpu_percent_from_deltas` | `(cpu_delta / system_delta) × cores × 100` |
| `clamps_cpu_percent_on_counter_reset` | A counter reset would underflow and render as an absurd spike |
| `subtracts_page_cache_from_memory_usage` | Matches `docker stats`; without it the number reads far higher than the CLI's |
| `tolerates_stats_with_no_cpu_or_memory` | A frame with everything absent must yield zeros, not panic |

**[`runtime/compose.rs`](../tauri-rs/src-tauri/src/runtime/compose.rs)**

| Test | Why it exists |
|---|---|
| `parses_jsonl_output` | Compose v2 emits one JSON object per line |
| `parses_array_output` | Some versions and podman-compose emit a single array |
| `falls_back_to_project_name` | Directory name when the output omits `Project` |
| `ignores_garbage_lines` | Warnings on stdout must not break parsing |

**[`commands.rs`](../tauri-rs/src-tauri/src/commands.rs)** — exec framing

| Test | Why it exists |
|---|---|
| `base64_round_trips_arbitrary_terminal_bytes` | Terminal traffic is not text. Colour escapes, `Ctrl-C` (`0x03`), multi-byte UTF-8 and byte sequences that are not valid UTF-8 at all must survive unaltered |
| `base64_preserves_split_utf8_sequences` | A character split across a chunk boundary is normal in a byte stream; encoding must not "repair" either half |
| `rejects_malformed_payload` | A bad payload is `kind == "invalid"`, not a panic |

**[`state.rs`](../tauri-rs/src-tauri/src/state.rs)** — exec session registry

| Test | Why it exists |
|---|---|
| `write_reaches_the_process_verbatim` | Keystrokes arrive byte-for-byte, including control bytes and multi-byte characters — nothing may line-buffer or re-encode |
| `unknown_session_is_not_found` | A closed terminal yields `not_found`, not a panic on a missing key |
| `stop_exec_drops_the_session` | Teardown removes both halves and is idempotent |
| `stop_all_streams_clears_exec_sessions` | Regression: exec registers in two maps, so a runtime switch that aborted only the pump task would strand its stdin writer against the abandoned daemon |

## Integration tests

[`tests/common/mod.rs`](../tauri-rs/src-tauri/tests/common/mod.rs) holds the
assertions; [`live_docker.rs`](../tauri-rs/src-tauri/tests/live_docker.rs) and
[`live_podman.rs`](../tauri-rs/src-tauri/tests/live_podman.rs) run them against a
**real daemon**. These cover what unit tests cannot: that our bollard calls are
shaped correctly and that the mapping onto our DTOs holds against real payloads
from *both* runtimes.

> **What these do to your machine.** They create volumes, networks, and
> short-lived containers, and remove them in the same test — including on the
> failure paths. Everything is prefixed `cleat-test-`, and names are namespaced
> per runtime so a Docker run and a Podman run can overlap. Containers are built
> from an image already present locally; **if the runtime's image store is empty
> the suite pulls `alpine:latest` (~9 MB) once**, which is the only case where it
> touches the network. Nothing pre-existing is modified or deleted.

> **Without a daemon they skip, not fail.** Each file's `rt()` helper returns
> `None` and prints why, so `cargo test` passes on a machine with neither runtime
> installed.

| Test | Covers |
|---|---|
| `reports_system_summary` | `/version` + `/info`, populated version and CPU count |
| `lists_containers_with_mapped_fields` | Running ⊆ all *by id*; names have the leading `/` stripped; state enum deserialises (not `"unknown"`). Samples `running` before `all` and retries once: the two calls cannot be atomic while the rest of the suite creates containers on other threads, and comparing counts the other way round fails whenever one starts in between |
| `lists_images_with_sizes` | Non-empty IDs, non-negative sizes |
| `lists_networks` | At least one default network; id, name, driver all populated |
| `volume_roundtrip` | Create → appears in list → inspect → remove → gone |
| `network_roundtrip` | Same, for networks |
| `container_lifecycle` | Create → list → inspect → health → start → appears running → logs → stop → remove → gone |
| `maps_error_kinds` | 404 maps to `kind == "not_found"`, not a generic failure |
| `rejects_injection_in_service_control` | Metacharacter service name and unknown action both rejected **before** reaching the daemon |
| `rejects_bad_compose_project_dir` | Nonexistent directory fails validation rather than running somewhere unexpected |
| `stats_stream_reports_cpu` | A busy-looping container must report non-zero CPU across samples — the canary for a missing `precpu_stats` |
| `log_follow_delivers_output` | A known marker written by the container must arrive on the follow stream |
| `exec_roundtrip` | The only two-way path: a command written to the session's stdin must come back on its output stream, through a real TTY |
| `exec_resize` | Resize is a separate endpoint addressed by exec id, and Podman serves it from its compatibility layer — the likeliest place the two runtimes diverge. Also pins that degenerate sizes are clamped rather than sent |
| `detects_a_usable_shell` | The probe must name an absolute path that is actually executable in the image; assuming `/bin/bash` fails on Alpine and most slim images |
| `rejects_empty_exec_command` | Empty and whitespace-only argv are refused locally rather than becoming an opaque daemon error |

### Podman-specific tests

Beyond the shared suite, [`live_podman.rs`](../tauri-rs/src-tauri/tests/live_podman.rs) adds:

| Test | Covers |
|---|---|
| `identifies_as_podman` | `kind()` and the system summary both report Podman |
| `rejects_overlay_network_driver` | netavark has no overlay driver; rejected with a reason naming it |
| `uses_a_podman_compose_frontend` | `podman compose` or `podman-compose`, never Docker's |
| `is_not_the_docker_daemon` | Podman and Docker report different versions — regression for the [bollard fallback](runtimes.md#footgun-bollards-podman-defaults-fall-back-to-docker) |
| `copies_an_image_from_docker` | A real image streams Docker → Podman via export/import and appears in Podman afterwards |

`live_docker.rs` correspondingly asserts `uses_compose_v2` — `docker compose`,
not the EOL v1 binary.

### Why `maps_error_kinds` and the rejection tests are integration tests

They assert on *error kinds*, which only exist once a request has been through the
real error-mapping path. Testing rejection in isolation would prove the validator
works but not that the validator is actually wired in ahead of the daemon call.

## Frontend

`npx tsc --noEmit` in `tauri-rs/` typechecks under `strict` with `noUnusedLocals`
and `noUnusedParameters`. There is no component test suite yet — the frontend is
thin over `api.ts`, and the logic worth pinning lives in Rust.

### Compose streaming

[`compose_timing.rs`](../tauri-rs/src-tauri/tests/compose_timing.rs) pins the fix
for a report that `Down` appeared to hang. It never did — Docker waits a
10-second SIGTERM grace period per stubborn container — but the blocking call
showed nothing for the duration.

| Test | Covers |
|---|---|
| `compose_down_streams_output_and_terminates` | Against a container that ignores SIGTERM: first output arrives strictly before completion (measured ~66 ms vs ~10.5 s total), and the command terminates |
| `compose_down_rejects_a_service_argument` | `down` is project-wide; naming a service is rejected |
| `compose_exec_rejects_injected_service_name` | Metacharacters never reach argv |

## What the shared suite caught

Running identical assertions against both runtimes found two real defects that
single-runtime testing would have missed:

1. **Empty health status on Podman.** Docker omits `State.Health` for a container
   with no healthcheck; Podman includes it with an empty string. The UI keys off
   `health !== "none"`, so every Podman container rendered a blank health badge.
   Fixed by `normalize_health()`.
2. **`create_container` whitespace-split its command.** `sh -c "while true; do :;
   done"` became seven arguments, so `-c` received only `while` and the container
   exited instantly. The field is now exact argv (`Vec<String>`), which also
   removes a footgun for the create-container form: a string that *looks* shell-like
   but silently isn't. Caught by `stats_stream_reports_cpu`, which noticed a
   supposedly busy-looping container using 0% CPU.

## Gaps worth knowing

- **Podman coverage is Linux rootless only.** Verified on Podman 4.9.3, rootless,
  cgroups v2. Not exercised: rootful Podman, cgroups v1, `podman-compose` as the
  provider, or Podman on macOS (where it runs in a VM).
- **Image-copy progress events have no dedicated test.** The transfer itself is
  covered by `copies_an_image_from_docker`, but the throttled byte-progress
  events are not asserted on.
- **Pull progress has no dedicated test.** `ensure_image()` drives a real pull
  when the store is empty, so the streaming path does execute, but no test
  asserts on the progress events themselves.
- **Compose commands are not integration-tested** beyond directory validation —
  doing so needs a fixture project and would start real containers.
- **No frontend component tests.** See [Frontend](#frontend) above.

## Adding tests

Unit tests go next to the code. For integration tests, follow the existing
pattern: add the assertion to `common::suite` so **both** runtimes get it, take
`&dyn ContainerRuntime`, prefix resources `cleat-test-`, clean up before
asserting (so a failed assertion cannot leak a resource), and prefer an
already-present image over pulling one.
