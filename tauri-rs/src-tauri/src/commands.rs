//! Tauri command layer.
//!
//! Replaces the Electron app's localhost Express server. That server bound
//! 0.0.0.0:3000 with permissive CORS, which handed full Docker daemon control
//! (root-equivalent) to anything on the LAN. These are IPC commands: there is
//! no socket, no port, and no origin to get wrong.

use crate::error::{AppError, AppResult};
use crate::model::{
    ActivityEntry, ComposeService, Container, CreateContainerRequest, Image, Network, RuntimeInfo,
    RuntimeKind, Stats, SystemSummary, Volume,
};
use crate::runtime;
use crate::state::AppState;
use futures_util::StreamExt;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

// ============================================================ runtime / system

#[tauri::command]
pub async fn list_runtimes() -> Vec<RuntimeInfo> {
    runtime::detect_runtimes().await
}

#[tauri::command]
pub async fn current_runtime(state: State<'_, AppState>) -> AppResult<Option<RuntimeKind>> {
    Ok(state.current_kind().await)
}

#[tauri::command]
pub async fn select_runtime(state: State<'_, AppState>, kind: RuntimeKind) -> AppResult<()> {
    state.select_runtime(kind).await
}

#[tauri::command]
pub async fn system_summary(state: State<'_, AppState>) -> AppResult<SystemSummary> {
    state.runtime().await?.system_summary().await
}

// ================================================================== containers

#[tauri::command]
pub async fn list_containers(state: State<'_, AppState>, all: bool) -> AppResult<Vec<Container>> {
    state.runtime().await?.list_containers(all).await
}

#[tauri::command]
pub async fn inspect_container(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<serde_json::Value> {
    state.runtime().await?.inspect_container(&id).await
}

#[tauri::command]
pub async fn start_container(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.runtime().await?.start_container(&id).await
}

#[tauri::command]
pub async fn stop_container(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.runtime().await?.stop_container(&id).await
}

#[tauri::command]
pub async fn restart_container(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.runtime().await?.restart_container(&id).await
}

#[tauri::command]
pub async fn pause_container(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.runtime().await?.pause_container(&id).await
}

#[tauri::command]
pub async fn unpause_container(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.runtime().await?.unpause_container(&id).await
}

#[tauri::command]
pub async fn remove_container(
    state: State<'_, AppState>,
    id: String,
    force: bool,
    volumes: bool,
) -> AppResult<()> {
    state
        .runtime()
        .await?
        .remove_container(&id, force, volumes)
        .await
}

#[tauri::command]
pub async fn create_container(
    state: State<'_, AppState>,
    req: CreateContainerRequest,
) -> AppResult<String> {
    state.runtime().await?.create_container(req).await
}

#[tauri::command]
pub async fn container_health(state: State<'_, AppState>, id: String) -> AppResult<String> {
    state.runtime().await?.container_health(&id).await
}

#[tauri::command]
pub async fn container_logs(
    state: State<'_, AppState>,
    id: String,
    tail: Option<i64>,
    timestamps: Option<bool>,
) -> AppResult<String> {
    state
        .runtime()
        .await?
        .container_logs(&id, tail.unwrap_or(200), timestamps.unwrap_or(false))
        .await
}

#[tauri::command]
pub async fn container_stats(state: State<'_, AppState>, id: String) -> AppResult<Stats> {
    state.runtime().await?.container_stats(&id).await
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServiceEntry {
    pub name: String,
    pub state: String,
}

#[tauri::command]
pub async fn list_container_services(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<Vec<ServiceEntry>> {
    let units = state.runtime().await?.list_container_services(&id).await?;
    Ok(units
        .into_iter()
        .map(|(name, state)| ServiceEntry { name, state })
        .collect())
}

#[tauri::command]
pub async fn control_container_service(
    state: State<'_, AppState>,
    id: String,
    service: String,
    action: String,
) -> AppResult<String> {
    state
        .runtime()
        .await?
        .control_container_service(&id, &service, &action)
        .await
}

// ====================================================================== images

#[tauri::command]
pub async fn list_images(state: State<'_, AppState>) -> AppResult<Vec<Image>> {
    state.runtime().await?.list_images().await
}

#[tauri::command]
pub async fn remove_image(state: State<'_, AppState>, id: String, force: bool) -> AppResult<()> {
    state.runtime().await?.remove_image(&id, force).await
}

#[tauri::command]
pub async fn inspect_image(state: State<'_, AppState>, id: String) -> AppResult<serde_json::Value> {
    state.runtime().await?.inspect_image(&id).await
}

#[tauri::command]
pub async fn image_history(state: State<'_, AppState>, id: String) -> AppResult<serde_json::Value> {
    state.runtime().await?.image_history(&id).await
}

#[tauri::command]
pub async fn prune_images(state: State<'_, AppState>) -> AppResult<u64> {
    state.runtime().await?.prune_images().await
}

/// Registries the active runtime has a login for.
///
/// Names and sources only — never a secret. Credential helpers are not invoked,
/// so opening the panel cannot make the OS prompt for a keychain unlock.
#[tauri::command]
pub async fn registry_logins(
    state: State<'_, AppState>,
) -> AppResult<Vec<runtime::credentials::RegistryLogin>> {
    let kind = state
        .current_kind()
        .await
        .ok_or_else(|| AppError::RuntimeUnavailable("no runtime selected".into()))?;
    Ok(runtime::credentials::list_logins(kind))
}

/// Which registry `image` would authenticate against, and as whom.
///
/// Lets the pull dialog say "as alice" before starting, so a 401 on a private
/// image is diagnosable without reading daemon logs. Reports the identity, not
/// the credential.
#[tauri::command]
pub async fn registry_identity(
    state: State<'_, AppState>,
    image: String,
) -> AppResult<runtime::credentials::RegistryLogin> {
    let kind = state
        .current_kind()
        .await
        .ok_or_else(|| AppError::RuntimeUnavailable("no runtime selected".into()))?;
    Ok(runtime::credentials::identity_for(kind, &image))
}

// ==================================================================== networks

#[tauri::command]
pub async fn list_networks(state: State<'_, AppState>) -> AppResult<Vec<Network>> {
    state.runtime().await?.list_networks().await
}

#[tauri::command]
pub async fn create_network(
    state: State<'_, AppState>,
    name: String,
    driver: String,
    internal: Option<bool>,
) -> AppResult<String> {
    state
        .runtime()
        .await?
        .create_network(&name, &driver, internal.unwrap_or(false))
        .await
}

#[tauri::command]
pub async fn remove_network(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.runtime().await?.remove_network(&id).await
}

#[tauri::command]
pub async fn inspect_network(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<serde_json::Value> {
    state.runtime().await?.inspect_network(&id).await
}

#[tauri::command]
pub async fn connect_network(
    state: State<'_, AppState>,
    network: String,
    container: String,
) -> AppResult<()> {
    state
        .runtime()
        .await?
        .connect_network(&network, &container)
        .await
}

#[tauri::command]
pub async fn disconnect_network(
    state: State<'_, AppState>,
    network: String,
    container: String,
) -> AppResult<()> {
    state
        .runtime()
        .await?
        .disconnect_network(&network, &container)
        .await
}

// ===================================================================== volumes

#[tauri::command]
pub async fn list_volumes(state: State<'_, AppState>) -> AppResult<Vec<Volume>> {
    state.runtime().await?.list_volumes().await
}

/// Bytes on disk per volume name.
///
/// Its own command rather than a field on `list_volumes` because the daemon
/// only computes volume sizes in its disk-usage report, which walks every
/// volume and takes seconds; the listing polls, this does not.
#[tauri::command]
pub async fn volume_usage(state: State<'_, AppState>) -> AppResult<HashMap<String, i64>> {
    state.runtime().await?.volume_usage().await
}

#[tauri::command]
pub async fn create_volume(
    state: State<'_, AppState>,
    name: String,
    driver: Option<String>,
) -> AppResult<Volume> {
    state
        .runtime()
        .await?
        .create_volume(&name, driver.as_deref())
        .await
}

#[tauri::command]
pub async fn remove_volume(
    state: State<'_, AppState>,
    name: String,
    force: Option<bool>,
) -> AppResult<()> {
    state
        .runtime()
        .await?
        .remove_volume(&name, force.unwrap_or(false))
        .await
}

#[tauri::command]
pub async fn inspect_volume(
    state: State<'_, AppState>,
    name: String,
) -> AppResult<serde_json::Value> {
    state.runtime().await?.inspect_volume(&name).await
}

#[tauri::command]
pub async fn prune_volumes(state: State<'_, AppState>) -> AppResult<u64> {
    state.runtime().await?.prune_volumes().await
}

// ===================================================================== compose

#[tauri::command]
pub async fn compose_services(
    state: State<'_, AppState>,
    project_path: String,
) -> AppResult<Vec<ComposeService>> {
    state.runtime().await?.compose_services(&project_path).await
}

#[tauri::command]
pub async fn compose_up(state: State<'_, AppState>, project_path: String) -> AppResult<String> {
    state.runtime().await?.compose_up(&project_path).await
}

#[tauri::command]
pub async fn compose_down(state: State<'_, AppState>, project_path: String) -> AppResult<String> {
    state.runtime().await?.compose_down(&project_path).await
}

#[tauri::command]
pub async fn compose_restart(
    state: State<'_, AppState>,
    project_path: String,
    service: Option<String>,
) -> AppResult<String> {
    state
        .runtime()
        .await?
        .compose_restart(&project_path, service.as_deref())
        .await
}

// =================================================================== streaming
//
// Each of these spawns a task that pumps a runtime stream onto a Tauri event
// channel and registers it so it can be cancelled. The Electron app had none
// of this: logs were a one-shot `tail 100`, stats a single sample, and pull
// progress was logged server-side where the UI could never see it.

/// Terminal frame for every stream channel, so the UI always learns why a
/// stream ended rather than just going quiet.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StreamEnd {
    channel: String,
    error: Option<String>,
}

fn emit_end(app: &AppHandle, channel: &str, error: Option<String>) {
    let _ = app.emit(
        &format!("{channel}:end"),
        StreamEnd {
            channel: channel.to_string(),
            error,
        },
    );
}

#[tauri::command]
pub async fn follow_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    tail: Option<i64>,
    channel: String,
) -> AppResult<()> {
    let rt = state.runtime().await?;
    let mut stream = rt.follow_logs(&id, tail.unwrap_or(200)).await?;

    let handle = tokio::spawn(async move {
        while let Some(item) = stream.next().await {
            match item {
                Ok(line) => {
                    if app.emit(&channel, line).is_err() {
                        break; // window gone
                    }
                }
                Err(e) => {
                    emit_end(&app, &channel, Some(e.message()));
                    return;
                }
            }
        }
        emit_end(&app, &channel, None);
    });

    state.register_stream(format!("logs:{id}"), handle).await;
    Ok(())
}

#[tauri::command]
pub async fn stop_follow_logs(state: State<'_, AppState>, id: String) -> AppResult<bool> {
    Ok(state.stop_stream(&format!("logs:{id}")).await)
}

// ==================================================================== exec
//
// The only bidirectional path in the app. Output is pumped onto `channel` like
// any other stream; input arrives one `exec_write` call at a time and is
// routed to the session's parked stdin writer.
//
// Terminal traffic is base64 on both legs. Tauri events are JSON, so raw bytes
// would serialise as an array of integers — several times larger than base64 —
// and a `String` would force a lossy UTF-8 decode that corrupts any multi-byte
// character split across a chunk boundary and mangles escape sequences.

/// Probe the container for a shell that exists, so the UI can offer a sensible
/// default instead of failing on every image without bash.
#[tauri::command]
pub async fn detect_shell(state: State<'_, AppState>, id: String) -> AppResult<String> {
    let rt = state.runtime().await?;
    rt.detect_shell(&id).await
}

#[tauri::command]
pub async fn exec_start(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    argv: Vec<String>,
    cols: u16,
    rows: u16,
    channel: String,
) -> AppResult<()> {
    let rt = state.runtime().await?;
    let attach = rt.exec_start(&id, argv, cols, rows).await?;
    let runtime::ExecAttach {
        exec_id,
        mut output,
        stdin,
    } = attach;

    // Register stdin before the pump starts: a fast-exiting process could
    // otherwise emit its end frame while the frontend still has no session to
    // tear down.
    state.register_exec(channel.clone(), exec_id, stdin).await;

    let pump_channel = channel.clone();
    let handle = tokio::spawn(async move {
        while let Some(item) = output.next().await {
            match item {
                Ok(bytes) => {
                    if app.emit(&pump_channel, b64(&bytes)).is_err() {
                        break; // window gone
                    }
                }
                Err(e) => {
                    emit_end(&app, &pump_channel, Some(e.message()));
                    return;
                }
            }
        }
        // The stream ending means the process exited or the connection dropped.
        emit_end(&app, &pump_channel, None);
    });

    state
        .register_stream(format!("exec:{channel}"), handle)
        .await;
    Ok(())
}

/// Forward keystrokes. `data` is base64.
#[tauri::command]
pub async fn exec_write(
    state: State<'_, AppState>,
    session: String,
    data: String,
) -> AppResult<()> {
    let bytes = unb64(&data)?;
    state.exec(&session).await?.write(&bytes).await
}

#[tauri::command]
pub async fn exec_resize(
    state: State<'_, AppState>,
    session: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    let rt = state.runtime().await?;
    let exec_id = state.exec(&session).await?.exec_id.clone();
    rt.exec_resize(&exec_id, cols, rows).await
}

#[tauri::command]
pub async fn exec_stop(state: State<'_, AppState>, session: String) -> AppResult<bool> {
    Ok(state.stop_exec(&session).await)
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(s: &str) -> AppResult<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| AppError::Invalid(format!("malformed exec payload: {e}")))
}

#[cfg(test)]
mod exec_tests {
    use super::*;

    /// Terminal traffic is not text. Escape sequences, control bytes and bytes
    /// that are not valid UTF-8 at all have to survive the round trip intact —
    /// this is the reason the channel is base64 rather than a `String`.
    #[test]
    fn base64_round_trips_arbitrary_terminal_bytes() {
        let cases: Vec<Vec<u8>> = vec![
            b"ls -la\n".to_vec(),
            b"\x1b[31mred\x1b[0m".to_vec(),     // colour escape
            b"\x03".to_vec(),                   // Ctrl-C
            "café ☕".as_bytes().to_vec(),      // multi-byte UTF-8
            vec![0x00, 0x01, 0x02, 0xff, 0xfe], // not valid UTF-8
            vec![],                             // empty
        ];

        for original in cases {
            let decoded = unb64(&b64(&original)).expect("decode");
            assert_eq!(decoded, original, "round trip altered {original:?}");
        }
    }

    /// A partial UTF-8 sequence at a chunk boundary is normal in a byte stream.
    /// Encoding must not "fix" it — the frontend reassembles across chunks.
    #[test]
    fn base64_preserves_split_utf8_sequences() {
        let full = "é".as_bytes().to_vec(); // two bytes
        let (head, tail) = full.split_at(1);
        assert_eq!(unb64(&b64(head)).unwrap(), head);
        assert_eq!(unb64(&b64(tail)).unwrap(), tail);
    }

    #[test]
    fn rejects_malformed_payload() {
        let err = unb64("not!valid!base64").expect_err("should reject");
        assert_eq!(err.kind(), "invalid");
    }
}

#[tauri::command]
pub async fn stream_stats(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    channel: String,
) -> AppResult<()> {
    let rt = state.runtime().await?;
    let mut stream = rt.stream_stats(&id).await?;

    let handle = tokio::spawn(async move {
        while let Some(item) = stream.next().await {
            match item {
                Ok(sample) => {
                    if app.emit(&channel, sample).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    emit_end(&app, &channel, Some(e.message()));
                    return;
                }
            }
        }
        emit_end(&app, &channel, None);
    });

    state.register_stream(format!("stats:{id}"), handle).await;
    Ok(())
}

#[tauri::command]
pub async fn stop_stream_stats(state: State<'_, AppState>, id: String) -> AppResult<bool> {
    Ok(state.stop_stream(&format!("stats:{id}")).await)
}

#[tauri::command]
pub async fn pull_image(
    app: AppHandle,
    state: State<'_, AppState>,
    image: String,
    channel: String,
) -> AppResult<()> {
    let rt = state.runtime().await?;
    let mut stream = rt.pull_image(&image).await?;
    let label = image.clone();

    let handle = tokio::spawn(async move {
        let mut failure: Option<String> = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(mut progress) => {
                    // The daemon reports pull failures in-band on a 200 stream,
                    // so an `error` field here is a real failure, not a warning.
                    if let Some(err) = progress.error.clone() {
                        failure = Some(err);
                    }
                    progress.done = false;
                    if app.emit(&channel, progress).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    failure = Some(e.message());
                    break;
                }
            }
        }

        let _ = app.emit(
            &channel,
            crate::model::PullProgress {
                image: label,
                id: None,
                status: if failure.is_some() {
                    "failed".into()
                } else {
                    "complete".into()
                },
                current: None,
                total: None,
                overall: if failure.is_some() { None } else { Some(1.0) },
                done: true,
                error: failure.clone(),
            },
        );
        emit_end(&app, &channel, failure);
    });

    state.register_stream(format!("pull:{image}"), handle).await;
    Ok(())
}

#[tauri::command]
pub async fn stop_pull(state: State<'_, AppState>, image: String) -> AppResult<bool> {
    Ok(state.stop_stream(&format!("pull:{image}")).await)
}

#[tauri::command]
pub async fn tag_image(
    state: State<'_, AppState>,
    source: String,
    target: String,
) -> AppResult<()> {
    state.runtime().await?.tag_image(&source, &target).await
}

/// Push `image`, streaming the daemon's progress onto `channel`.
///
/// Structurally identical to [`pull_image`] — same in-band failure handling,
/// same registration so it can be cancelled — because it is the same kind of
/// long-running one-way stream. The one difference is that push events carry no
/// overall fraction; see `Engine::push_image`.
#[tauri::command]
pub async fn push_image(
    app: AppHandle,
    state: State<'_, AppState>,
    image: String,
    channel: String,
) -> AppResult<()> {
    let rt = state.runtime().await?;
    let mut stream = rt.push_image(&image).await?;
    let label = image.clone();

    let handle = tokio::spawn(async move {
        let mut failure: Option<String> = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(mut progress) => {
                    // A 200 response can still carry a failure in-band — a
                    // denied push reports `errorDetail` and then simply stops.
                    if let Some(err) = progress.error.clone() {
                        failure = Some(err);
                    }
                    progress.done = false;
                    if app.emit(&channel, progress).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    failure = Some(e.message());
                    break;
                }
            }
        }

        let _ = app.emit(
            &channel,
            crate::model::PullProgress {
                image: label,
                id: None,
                status: if failure.is_some() {
                    "failed".into()
                } else {
                    "pushed".into()
                },
                current: None,
                total: None,
                overall: if failure.is_some() { None } else { Some(1.0) },
                done: true,
                error: failure.clone(),
            },
        );
        emit_end(&app, &channel, failure);
    });

    state.register_stream(format!("push:{image}"), handle).await;
    Ok(())
}

#[tauri::command]
pub async fn stop_push(state: State<'_, AppState>, image: String) -> AppResult<bool> {
    Ok(state.stop_stream(&format!("push:{image}")).await)
}

/// Emit at most one progress event per this many bytes.
///
/// Chunks arrive at tens of kB, so emitting per chunk would push thousands of
/// events through the channel for a single image.
const PROGRESS_STEP: u64 = 4 * 1024 * 1024;

/// Build one transfer progress event.
#[allow(clippy::too_many_arguments)]
fn transfer_event(
    image: &str,
    from: RuntimeKind,
    to: RuntimeKind,
    total: Option<i64>,
    phase: &str,
    bytes: u64,
    status: Option<String>,
    done: bool,
    error: Option<String>,
) -> crate::model::ImageTransferProgress {
    crate::model::ImageTransferProgress {
        image: image.to_string(),
        from,
        to,
        phase: phase.to_string(),
        bytes,
        total,
        overall: if done && error.is_none() {
            Some(1.0)
        } else {
            total
                .filter(|t| *t > 0)
                .map(|t| (bytes as f64 / t as f64).clamp(0.0, 1.0))
        },
        status,
        done,
        error,
    }
}

/// Copy an image from the active runtime into another one.
///
/// Docker and Podman keep entirely separate image stores, so "have this image
/// on both" means physically transferring it. The source's export stream feeds
/// the destination's import directly: a multi-gigabyte image never lands on
/// disk or accumulates in memory.
#[tauri::command]
pub async fn copy_image(
    app: AppHandle,
    state: State<'_, AppState>,
    image: String,
    to: RuntimeKind,
    channel: String,
) -> AppResult<()> {
    let source = state.runtime().await?;
    let from = source.kind();
    if from == to {
        return Err(AppError::Invalid(
            "source and destination runtimes are the same".into(),
        ));
    }

    let reference = image.trim().to_string();
    if reference.is_empty() {
        return Err(AppError::Invalid("image reference is required".into()));
    }

    // Connect to the destination before starting the export, so an unreachable
    // target fails immediately rather than after streaming gigabytes.
    let dest = runtime::connect(to).await?;

    // Size only drives the percentage; an unknown size still transfers fine.
    let total = source
        .list_images()
        .await
        .ok()
        .and_then(|images| {
            images
                .into_iter()
                .find(|i| i.repo_tags.iter().any(|t| *t == reference) || i.id == reference)
        })
        .map(|i| i.size);

    let export = source.export_image(&reference).await?;

    let _ = app.emit(
        &channel,
        transfer_event(
            &reference,
            from,
            to,
            total,
            "transferring",
            0,
            None,
            false,
            None,
        ),
    );

    let counter = Arc::new(AtomicU64::new(0));
    let last_emitted = Arc::new(AtomicU64::new(0));

    let counted = {
        let app = app.clone();
        let channel = channel.clone();
        let counter = counter.clone();
        let reference = reference.clone();
        export.map(move |chunk| {
            if let Ok(bytes) = &chunk {
                let sent =
                    counter.fetch_add(bytes.len() as u64, Ordering::Relaxed) + bytes.len() as u64;
                if sent.saturating_sub(last_emitted.load(Ordering::Relaxed)) >= PROGRESS_STEP {
                    last_emitted.store(sent, Ordering::Relaxed);
                    let _ = app.emit(
                        &channel,
                        transfer_event(
                            &reference,
                            from,
                            to,
                            total,
                            "transferring",
                            sent,
                            None,
                            false,
                            None,
                        ),
                    );
                }
            }
            chunk
        })
    };

    let mut import = dest.import_image(Box::pin(counted)).await?;

    // The stream key needs the reference after the task takes ownership.
    let stream_key = format!("copy:{reference}:{}", to.as_str());

    let handle = tokio::spawn(async move {
        let mut failure: Option<String> = None;
        while let Some(item) = import.next().await {
            match item {
                Ok(status) if status.is_empty() => {}
                Ok(status) => {
                    let sent = counter.load(Ordering::Relaxed);
                    let _ = app.emit(
                        &channel,
                        transfer_event(
                            &reference,
                            from,
                            to,
                            total,
                            "importing",
                            sent,
                            Some(status),
                            false,
                            None,
                        ),
                    );
                }
                Err(e) => {
                    failure = Some(e.message());
                    break;
                }
            }
        }

        let sent = counter.load(Ordering::Relaxed);
        let _ = app.emit(
            &channel,
            transfer_event(
                &reference,
                from,
                to,
                total,
                if failure.is_some() {
                    "failed"
                } else {
                    "complete"
                },
                sent,
                None,
                true,
                failure.clone(),
            ),
        );
        emit_end(&app, &channel, failure);
    });

    state.register_stream(stream_key, handle).await;
    Ok(())
}

#[tauri::command]
pub async fn stop_copy_image(
    state: State<'_, AppState>,
    image: String,
    to: RuntimeKind,
) -> AppResult<bool> {
    Ok(state
        .stop_stream(&format!("copy:{}:{}", image.trim(), to.as_str()))
        .await)
}

/// Run a compose action with live output.
///
/// Replaces the blocking `compose_up`/`compose_down` for anything the UI drives:
/// Docker waits a 10-second SIGTERM grace period per container that ignores it,
/// so a multi-service `down` can take a minute. Streaming the output means the
/// user sees which container is stopping instead of an inert spinner.
#[tauri::command]
pub async fn compose_exec(
    app: AppHandle,
    state: State<'_, AppState>,
    project_path: String,
    action: crate::runtime::ComposeAction,
    service: Option<String>,
    channel: String,
) -> AppResult<()> {
    let rt = state.runtime().await?;
    let mut stream = rt
        .compose_exec(&project_path, action, service.as_deref())
        .await?;

    let key = format!("compose:{project_path}");
    let handle = tokio::spawn(async move {
        let mut failure: Option<String> = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(line) => {
                    if app.emit(&channel, line).is_err() {
                        return; // window gone; kill_on_drop stops the child
                    }
                }
                Err(e) => {
                    failure = Some(e.message());
                    break;
                }
            }
        }
        emit_end(&app, &channel, failure);
    });

    state.register_stream(key, handle).await;
    Ok(())
}

/// Cancel a running compose action. `kill_on_drop` terminates the child.
#[tauri::command]
pub async fn stop_compose_exec(
    state: State<'_, AppState>,
    project_path: String,
) -> AppResult<bool> {
    Ok(state.stop_stream(&format!("compose:{project_path}")).await)
}

/// Cancel every live stream, used when the UI navigates away wholesale.
#[tauri::command]
pub async fn stop_all_streams(state: State<'_, AppState>) -> AppResult<()> {
    state.stop_all_streams().await;
    Ok(())
}

/// Surface an unreachable-runtime failure with the same shape as other errors.
#[tauri::command]
pub async fn ping_runtime(state: State<'_, AppState>) -> AppResult<bool> {
    match state.runtime().await {
        Ok(rt) => {
            rt.system_summary().await?;
            Ok(true)
        }
        Err(AppError::NoRuntime(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

// ================================================================ activity

/// Everything Cleat has asked a runtime to do this session, newest first.
///
/// Cleat holds root-equivalent control of the daemon; this is how that claim is
/// shown rather than merely asserted. Recorded by the audit decorator in
/// `runtime::audit`, which wraps the runtime in `AppState::select_runtime` so
/// no operation can bypass it.
#[tauri::command]
pub async fn activity_log(state: State<'_, AppState>) -> AppResult<Vec<ActivityEntry>> {
    Ok(state.activity().snapshot())
}

#[tauri::command]
pub async fn clear_activity_log(state: State<'_, AppState>) -> AppResult<()> {
    state.activity().clear();
    Ok(())
}

// ============================================================== kubernetes

use crate::k8s;
use crate::k8s::generate;
use crate::k8s::manifests::ManifestOutcome;
use crate::k8s::model as k8s_model;
use crate::k8s::resources;

/// Contexts in the kubeconfig. Reads the file only; contacts no cluster.
#[tauri::command]
pub async fn k8s_contexts() -> AppResult<Vec<k8s_model::KubeContext>> {
    k8s::contexts()
}

/// Contact every context's cluster, concurrently, and report what answered.
#[tauri::command]
pub async fn k8s_probe_clusters() -> AppResult<Vec<k8s_model::ClusterInfo>> {
    k8s::probe_all().await
}

#[tauri::command]
pub async fn k8s_current_context(state: State<'_, AppState>) -> AppResult<Option<String>> {
    Ok(state.kube_context().await)
}

/// Choose a context. `None` falls back to whatever the kubeconfig marks current.
#[tauri::command]
pub async fn k8s_select_context(
    state: State<'_, AppState>,
    context: Option<String>,
) -> AppResult<()> {
    state.select_kube_context(context).await;
    Ok(())
}

#[tauri::command]
pub async fn k8s_list_namespaces(
    state: State<'_, AppState>,
) -> AppResult<Vec<k8s_model::Namespace>> {
    resources::list_namespaces(state.kube_client().await?).await
}

#[tauri::command]
pub async fn k8s_list_pods(
    state: State<'_, AppState>,
    namespace: Option<String>,
) -> AppResult<Vec<k8s_model::Pod>> {
    resources::list_pods(state.kube_client().await?, namespace.as_deref()).await
}

#[tauri::command]
pub async fn k8s_list_deployments(
    state: State<'_, AppState>,
    namespace: Option<String>,
) -> AppResult<Vec<k8s_model::Deployment>> {
    resources::list_deployments(state.kube_client().await?, namespace.as_deref()).await
}

#[tauri::command]
pub async fn k8s_list_services(
    state: State<'_, AppState>,
    namespace: Option<String>,
) -> AppResult<Vec<k8s_model::Service>> {
    resources::list_services(state.kube_client().await?, namespace.as_deref()).await
}

#[tauri::command]
pub async fn k8s_list_nodes(state: State<'_, AppState>) -> AppResult<Vec<k8s_model::Node>> {
    resources::list_nodes(state.kube_client().await?).await
}

#[tauri::command]
pub async fn k8s_inspect_pod(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
) -> AppResult<serde_json::Value> {
    resources::inspect_pod(state.kube_client().await?, &namespace, &name).await
}

/// Delete a pod. Its controller recreates it, which is how a workload is
/// restarted — there is no "restart pod" verb in the API.
#[tauri::command]
pub async fn k8s_delete_pod(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
) -> AppResult<()> {
    resources::delete_pod(state.kube_client().await?, &namespace, &name).await
}

#[tauri::command]
pub async fn k8s_pod_logs(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    container: Option<String>,
    tail: i64,
) -> AppResult<String> {
    resources::pod_logs(
        state.kube_client().await?,
        &namespace,
        &name,
        container.as_deref(),
        tail,
    )
    .await
}

/// Follow a pod's logs onto `channel`.
///
/// Same registration as the container path, so the same Stop button works and a
/// closed window drops the task. Keyed by namespace and name because pod names
/// are only unique within a namespace.
#[tauri::command]
pub async fn k8s_follow_pod_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    container: Option<String>,
    tail: i64,
    channel: String,
) -> AppResult<()> {
    let client = state.kube_client().await?;
    let mut stream =
        resources::follow_pod_logs(client, &namespace, &name, container.as_deref(), tail).await?;

    let key = format!("k8s-logs:{namespace}/{name}");
    let handle = tokio::spawn(async move {
        let mut failure: Option<String> = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(line) => {
                    if app.emit(&channel, line).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    failure = Some(e.message());
                    break;
                }
            }
        }
        emit_end(&app, &channel, failure);
    });

    state.register_stream(key, handle).await;
    Ok(())
}

#[tauri::command]
pub async fn k8s_stop_pod_logs(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
) -> AppResult<bool> {
    Ok(state
        .stop_stream(&format!("k8s-logs:{namespace}/{name}"))
        .await)
}

/// Apply a manifest, or find out what applying it would do.
///
/// `dry_run` is a server-side dry run: the API server runs validation, admission
/// webhooks and defaulting, then discards the result. The UI calls this first
/// and shows the outcomes before asking, so applying pasted YAML is a decision
/// rather than a gamble.
#[tauri::command]
pub async fn k8s_apply_manifest(
    state: State<'_, AppState>,
    yaml: String,
    namespace: Option<String>,
    dry_run: bool,
) -> AppResult<Vec<ManifestOutcome>> {
    k8s::manifests::apply(
        state.kube_client().await?,
        &yaml,
        namespace.as_deref(),
        dry_run,
    )
    .await
}

#[tauri::command]
pub async fn k8s_delete_manifest(
    state: State<'_, AppState>,
    yaml: String,
    namespace: Option<String>,
) -> AppResult<Vec<ManifestOutcome>> {
    k8s::manifests::delete(state.kube_client().await?, &yaml, namespace.as_deref()).await
}

#[tauri::command]
pub async fn k8s_ensure_namespace(state: State<'_, AppState>, name: String) -> AppResult<()> {
    k8s::manifests::ensure_namespace(state.kube_client().await?, &name).await
}

/// Generate a Deployment (and Service) from a running container.
///
/// Reads the container through the active runtime, so it works identically for
/// Docker and Podman, and returns YAML plus the warnings describing what the
/// translation could not carry across. Nothing is applied here.
#[tauri::command]
pub async fn k8s_generate_from_container(
    state: State<'_, AppState>,
    id: String,
    namespace: Option<String>,
    replicas: i32,
    include_service: bool,
) -> AppResult<generate::Generated> {
    let inspect = state.runtime().await?.inspect_container(&id).await?;
    generate::from_inspect(
        &inspect,
        &generate::Options {
            namespace,
            replicas,
            include_service,
        },
    )
}

/// Generate manifests for every container in a compose project.
///
/// One document set per container, concatenated. Compose services are already
/// separate workloads, so this is a Deployment each rather than one pod with
/// several containers — which is what `podman generate kube` does and what makes
/// its output hard to scale afterwards.
#[tauri::command]
pub async fn k8s_generate_from_compose(
    state: State<'_, AppState>,
    project: String,
    namespace: Option<String>,
    replicas: i32,
    include_service: bool,
) -> AppResult<generate::Generated> {
    let runtime = state.runtime().await?;
    let containers = runtime.list_containers(true).await?;
    let members: Vec<_> = containers
        .into_iter()
        .filter(|c| c.compose_project.as_deref() == Some(project.as_str()))
        .collect();

    if members.is_empty() {
        return Err(AppError::NotFound(format!(
            "no containers belong to compose project {project:?}"
        )));
    }

    let mut yaml = String::new();
    let mut warnings = Vec::new();
    for container in members {
        let inspect = runtime.inspect_container(&container.id).await?;
        let generated = generate::from_inspect(
            &inspect,
            &generate::Options {
                namespace: namespace.clone(),
                replicas,
                include_service,
            },
        )?;
        if !yaml.is_empty() {
            yaml.push_str("---\n");
        }
        yaml.push_str(&generated.yaml);
        warnings.extend(generated.warnings);
    }

    Ok(generate::Generated { yaml, warnings })
}

/// Recent cluster events, newest first.
///
/// The answer to "why is this pod Pending", which the pod object itself does
/// not carry.
#[tauri::command]
pub async fn k8s_list_events(
    state: State<'_, AppState>,
    namespace: Option<String>,
) -> AppResult<Vec<k8s_model::Event>> {
    resources::list_events(state.kube_client().await?, namespace.as_deref()).await
}

/// ConfigMaps and Secrets together, since they are read as one thing.
///
/// Secret *values* are never fetched — only which keys exist. See
/// `resources::list_secrets`.
#[tauri::command]
pub async fn k8s_list_config(
    state: State<'_, AppState>,
    namespace: Option<String>,
) -> AppResult<Vec<k8s_model::ConfigEntry>> {
    let client = state.kube_client().await?;
    let (maps, secrets) = tokio::join!(
        resources::list_configmaps(client.clone(), namespace.as_deref()),
        resources::list_secrets(client, namespace.as_deref())
    );
    let mut all = maps?;
    all.extend(secrets?);
    all.sort_by(|a, b| a.namespace.cmp(&b.namespace).then(a.name.cmp(&b.name)));
    Ok(all)
}

#[tauri::command]
pub async fn k8s_scale_deployment(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    replicas: i32,
) -> AppResult<()> {
    resources::scale_deployment(state.kube_client().await?, &namespace, &name, replicas).await
}

#[tauri::command]
pub async fn k8s_restart_deployment(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
) -> AppResult<()> {
    resources::restart_deployment(state.kube_client().await?, &namespace, &name).await
}

/// Open a shell inside a pod.
///
/// The cluster twin of [`exec_start`], and it reuses the same registries — the
/// stdin writer goes in `execs`, the output pump in `streams` — so the existing
/// teardown paths cover it. The resize channel is the one addition, because a
/// pod resize travels on the session's own socket rather than as its own call.
// Nine parameters because this is an IPC boundary, not a function anyone
// calls: every one is a distinct thing the frontend must name, and bundling
// them into a struct would only move the list into types.ts.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn k8s_exec_start(
    app: AppHandle,
    state: State<'_, AppState>,
    namespace: String,
    pod: String,
    container: Option<String>,
    argv: Vec<String>,
    cols: u16,
    rows: u16,
    channel: String,
) -> AppResult<()> {
    let client = state.kube_client().await?;
    let session = k8s::exec::attach(
        client,
        &namespace,
        &pod,
        container.as_deref(),
        argv,
        cols,
        rows,
    )
    .await?;

    let k8s::exec::PodExec {
        mut output,
        stdin,
        resize,
    } = session;

    // Registered before the pump starts, for the same reason the container path
    // does it: a shell that exits immediately would otherwise emit its end
    // frame while the frontend still has no session to tear down.
    state
        .register_exec(channel.clone(), format!("pod:{namespace}/{pod}"), stdin)
        .await;
    if let Some(tx) = resize {
        state.register_kube_resizer(channel.clone(), tx).await;
    }

    let pump_channel = channel.clone();
    let handle = tokio::spawn(async move {
        while let Some(item) = output.next().await {
            match item {
                Ok(bytes) => {
                    if app.emit(&pump_channel, b64(&bytes)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    emit_end(&app, &pump_channel, Some(e.message()));
                    return;
                }
            }
        }
        emit_end(&app, &pump_channel, None);
    });

    state
        .register_stream(format!("exec:{channel}"), handle)
        .await;
    Ok(())
}

#[tauri::command]
pub async fn k8s_exec_resize(
    state: State<'_, AppState>,
    channel: String,
    cols: u16,
    rows: u16,
) -> AppResult<bool> {
    Ok(state.resize_kube_exec(&channel, cols, rows).await)
}

#[tauri::command]
pub async fn k8s_exec_stop(state: State<'_, AppState>, channel: String) -> AppResult<bool> {
    state.drop_kube_resizer(&channel).await;
    // `stop_exec` clears both the session and its output task, which is the
    // same teardown the container terminal uses.
    Ok(state.stop_exec(&channel).await)
}

/// Forward a local port to a port on a pod.
///
/// Returns the port actually bound, which is what the caller needs when it
/// asks for 0 and lets the OS choose. Loopback only — see `k8s::exec`.
#[tauri::command]
pub async fn k8s_port_forward(
    state: State<'_, AppState>,
    namespace: String,
    pod: String,
    pod_port: u16,
    local_port: u16,
) -> AppResult<u16> {
    let client = state.kube_client().await?;
    let (bound, handle) =
        k8s::exec::port_forward(client, &namespace, &pod, pod_port, local_port).await?;
    state
        .register_stream(format!("k8s-forward:{namespace}/{pod}/{pod_port}"), handle)
        .await;
    Ok(bound)
}

#[tauri::command]
pub async fn k8s_stop_port_forward(
    state: State<'_, AppState>,
    namespace: String,
    pod: String,
    pod_port: u16,
) -> AppResult<bool> {
    Ok(state
        .stop_stream(&format!("k8s-forward:{namespace}/{pod}/{pod_port}"))
        .await)
}

/// A live resource as YAML, for editing.
///
/// Server-managed noise is stripped — `managedFields`, `resourceVersion`, `uid`,
/// `creationTimestamp`, `status` — because the point is a document a person can
/// read and change, and those fields are neither. What remains is what
/// `kubectl edit` would show you.
#[tauri::command]
pub async fn k8s_resource_yaml(
    state: State<'_, AppState>,
    api_version: String,
    kind: String,
    namespace: Option<String>,
    name: String,
) -> AppResult<String> {
    k8s::manifests::fetch_yaml(
        state.kube_client().await?,
        &api_version,
        &kind,
        namespace.as_deref(),
        &name,
    )
    .await
}

#[tauri::command]
pub async fn k8s_list_workloads(
    state: State<'_, AppState>,
    namespace: Option<String>,
) -> AppResult<Vec<k8s_model::Workload>> {
    resources::list_workloads(state.kube_client().await?, namespace.as_deref()).await
}
