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
    project_dir: String,
) -> AppResult<Vec<ComposeService>> {
    state.runtime().await?.compose_services(&project_dir).await
}

#[tauri::command]
pub async fn compose_up(state: State<'_, AppState>, project_dir: String) -> AppResult<String> {
    state.runtime().await?.compose_up(&project_dir).await
}

#[tauri::command]
pub async fn compose_down(state: State<'_, AppState>, project_dir: String) -> AppResult<String> {
    state.runtime().await?.compose_down(&project_dir).await
}

#[tauri::command]
pub async fn compose_restart(
    state: State<'_, AppState>,
    project_dir: String,
    service: Option<String>,
) -> AppResult<String> {
    state
        .runtime()
        .await?
        .compose_restart(&project_dir, service.as_deref())
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
    project_dir: String,
    action: crate::runtime::ComposeAction,
    service: Option<String>,
    channel: String,
) -> AppResult<()> {
    let rt = state.runtime().await?;
    let mut stream = rt
        .compose_exec(&project_dir, action, service.as_deref())
        .await?;

    let key = format!("compose:{project_dir}");
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
pub async fn stop_compose_exec(state: State<'_, AppState>, project_dir: String) -> AppResult<bool> {
    Ok(state.stop_stream(&format!("compose:{project_dir}")).await)
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
