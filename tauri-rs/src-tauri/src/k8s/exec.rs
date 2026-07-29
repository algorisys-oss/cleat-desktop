//! Interactive shells inside pods, and local port forwards.
//!
//! # Exec
//!
//! The cluster twin of [`Engine::exec_start`](crate::runtime::engine::Engine).
//! Same shape and same reasoning: raw bytes both ways, a TTY so full-screen
//! programs work, and a resize channel so they render at the right geometry.
//!
//! Two differences from the container path, both forced by the API:
//!
//! * **`stderr` must be off when `tty` is on.** A TTY is a single character
//!   device — there are not two streams to separate — and asking for both is
//!   rejected by the API server rather than merged.
//! * **Resize is a channel, not a call.** The container path addresses a resize
//!   to an exec id over HTTP; here the size is sent down the same WebSocket the
//!   session runs on, so the sender has to be kept alive alongside stdin.
//!
//! # Port forward
//!
//! `kubectl port-forward` in the app. A local TCP listener accepts connections
//! and pumps each one over a fresh forwarded stream. Bound to loopback only:
//! this punches a hole into a cluster network, and binding it to every
//! interface would offer that hole to the LAN.

use crate::error::{AppError, AppResult};
use crate::runtime::{ByteStream, ExecStdin};
use bytes::Bytes;
use futures_channel::mpsc;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{AttachParams, TerminalSize};
use kube::{Api, Client};

/// A live shell inside a pod.
pub struct PodExec {
    pub output: ByteStream,
    pub stdin: ExecStdin,
    /// Kept alive for the session's lifetime; dropping it ends resize support.
    pub resize: Option<mpsc::Sender<TerminalSize>>,
}

/// Shells to try, most featureful first.
///
/// Same probe the container path uses and for the same reason: a distroless or
/// Alpine image may have no bash, and failing with "no such file" instead of
/// falling back is a terminal that mysteriously refuses to open.
const SHELLS: [&str; 3] = ["/bin/bash", "/bin/sh", "/busybox/sh"];

/// Open a shell in `pod`.
///
/// `argv` empty means "probe for a shell". A non-empty argv is run verbatim,
/// which is the same deliberate exception the container terminal makes — see
/// docs/security.md. It is an argv vector to the API server, never a shell
/// string, so there is nothing to inject into.
pub async fn attach(
    client: Client,
    namespace: &str,
    pod: &str,
    container: Option<&str>,
    argv: Vec<String>,
    cols: u16,
    rows: u16,
) -> AppResult<PodExec> {
    let api: Api<Pod> = Api::namespaced(client, namespace);

    let mut params = AttachParams::interactive_tty();
    if let Some(c) = container {
        params = params.container(c.to_string());
    }

    let mut last_error = None;
    let candidates: Vec<Vec<String>> = if argv.is_empty() {
        SHELLS.iter().map(|s| vec![s.to_string()]).collect()
    } else {
        vec![argv]
    };

    for command in candidates {
        match api.exec(pod, command.clone(), &params).await {
            Ok(mut process) => {
                let stdin = process
                    .stdin()
                    .ok_or_else(|| AppError::Other("exec session has no stdin".into()))?;
                let stdout = process
                    .stdout()
                    .ok_or_else(|| AppError::Other("exec session has no stdout".into()))?;
                let resize = process.terminal_size();

                let mut resize = resize;
                if let Some(tx) = resize.as_mut() {
                    // Ignored on failure: a session that cannot be resized is
                    // still a usable session.
                    use futures_util::SinkExt as _;
                    let _ = tx
                        .send(TerminalSize {
                            width: cols,
                            height: rows,
                        })
                        .await;
                }

                return Ok(PodExec {
                    output: read_stream(stdout),
                    stdin: Box::pin(stdin),
                    resize,
                });
            }
            Err(e) => last_error = Some(e.to_string()),
        }
    }

    Err(AppError::Other(format!(
        "could not open a shell in {pod}: {}",
        last_error.unwrap_or_else(|| "no shell found".into())
    )))
}

/// Adapt an `AsyncRead` into the byte stream the command layer pumps.
///
/// Chunks are forwarded exactly as they arrive and never interpreted: this is
/// terminal output, so a split multi-byte character or a partial escape
/// sequence is normal and must survive to the other end intact.
fn read_stream(reader: impl tokio::io::AsyncRead + Unpin + Send + 'static) -> ByteStream {
    Box::pin(futures_util::stream::unfold(
        (reader, false),
        |(mut reader, done)| async move {
            if done {
                return None;
            }
            let mut buf = vec![0u8; 8192];
            match tokio::io::AsyncReadExt::read(&mut reader, &mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok(Bytes::from(buf)), (reader, false)))
                }
                // Yield the error once, then stop — looping would spin on a
                // broken pipe forever.
                Err(e) => Some((
                    Err(AppError::Other(format!("reading exec output: {e}"))),
                    (reader, true),
                )),
            }
        },
    ))
}

/// Forward a local port to a port on a pod.
///
/// Returns the port actually bound, which matters when the caller asks for 0
/// and lets the OS choose. The task runs until aborted through the stream
/// registry, exactly like the log and stats pumps.
pub async fn port_forward(
    client: Client,
    namespace: &str,
    pod: &str,
    pod_port: u16,
    local_port: u16,
) -> AppResult<(u16, tokio::task::JoinHandle<()>)> {
    // Loopback only, deliberately. A forward is a hole into a cluster network;
    // binding 0.0.0.0 would offer that hole to anything on the LAN, which is
    // the same class of mistake the Electron predecessor made with its HTTP
    // server. `kubectl` defaults to loopback for the same reason.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", local_port))
        .await
        .map_err(|e| AppError::Other(format!("binding 127.0.0.1:{local_port}: {e}")))?;
    let bound = listener
        .local_addr()
        .map_err(|e| AppError::Other(format!("reading bound port: {e}")))?
        .port();

    let api: Api<Pod> = Api::namespaced(client, namespace);
    let pod = pod.to_string();

    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut local, _)) = listener.accept().await else {
                return;
            };
            let api = api.clone();
            let pod = pod.clone();
            // One forwarded connection per accepted socket: a Portforwarder
            // carries a single stream, so sharing one across connections would
            // interleave two unrelated conversations.
            tokio::spawn(async move {
                let Ok(mut forwarder) = api.portforward(&pod, &[pod_port]).await else {
                    return;
                };
                let Some(mut upstream) = forwarder.take_stream(pod_port) else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut local, &mut upstream).await;
            });
        }
    });

    Ok((bound, handle))
}
