//! Lenient raw access to the Engine API over a unix socket.
//!
//! Exists because bollard deserialises into strictly-typed models generated
//! from Docker's OpenAPI spec, and **Podman is Docker-compatible, not
//! Docker-identical**. Podman reports a container state of `stopped`, which is
//! not in Docker's vocabulary (`created`, `running`, `paused`, `restarting`,
//! `exited`, `removing`, `dead`, `stopping`). serde rejects the unknown variant
//! and the *entire* `/containers/json` response fails to parse — so a single
//! stopped container blanked both the Containers and Networks views with a JSON
//! error.
//!
//! This module fetches the same endpoint into `serde_json::Value`, where an
//! unrecognised state is just a string. It is a fallback: the typed bollard path
//! is tried first and is what normally runs.
//!
//! Unix only, by construction. The transport is a unix-domain socket, and the
//! problem it solves is a Podman one — Docker never reports a state outside its
//! own vocabulary, and Docker is the only runtime reachable on Windows. See the
//! Windows [`get_json`] below.

use crate::error::{AppError, AppResult};
#[cfg(unix)]
use crate::runtime::wire;
#[cfg(unix)]
use http_body_util::{BodyExt, Full};
#[cfg(unix)]
use hyper::body::Bytes;
#[cfg(unix)]
use hyper_util::client::legacy::Client;
#[cfg(unix)]
use hyperlocal::{UnixClientExt, UnixConnector, Uri};

/// GET `path` from the daemon behind `socket`, parsed as untyped JSON.
#[cfg(unix)]
pub async fn get_json(socket: &str, path: &str) -> AppResult<serde_json::Value> {
    let client: Client<UnixConnector, Full<Bytes>> = Client::unix();

    // This client is ours, not bollard's, so bollard's capture hook never sees
    // it. Reporting it here keeps the activity log's claim — that it shows
    // every request — true for the lenient fallback too.
    wire::record(format!("GET {path}"));

    let uri: hyper::Uri = Uri::new(socket, path).into();
    let response = client
        .get(uri)
        .await
        .map_err(|e| AppError::Other(format!("raw request to {socket}{path} failed: {e}")))?;

    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|e| AppError::Other(format!("reading {path}: {e}")))?
        .to_bytes();

    if !status.is_success() {
        let message = String::from_utf8_lossy(&body).trim().to_string();
        return Err(match status.as_u16() {
            404 => AppError::NotFound(message),
            409 => AppError::Conflict(message),
            _ => AppError::Other(format!("{path} returned {status}: {message}")),
        });
    }

    serde_json::from_slice(&body).map_err(|e| AppError::Other(format!("parsing {path}: {e}")))
}

/// Unreachable on Windows, and deliberately so.
///
/// [`Engine::socket`](crate::runtime::engine::Engine) is the gate on every call
/// into here, and nothing sets it on Windows — there is no unix socket to set it
/// to. Kept as a signature rather than deleted so `engine.rs` stays one
/// codebase across platforms instead of growing `cfg` arms through its hot path.
#[cfg(windows)]
pub async fn get_json(socket: &str, path: &str) -> AppResult<serde_json::Value> {
    Err(AppError::Other(format!(
        "the lenient unix-socket fallback is not available on Windows \
         (requested {path} from {socket})"
    )))
}

/// Normalise a runtime-reported container state onto Docker's vocabulary.
///
/// Podman uses `stopped` where Docker uses `exited`; they mean the same thing,
/// and the UI colours and filters on this value.
pub fn normalize_state(raw: &str) -> String {
    match raw {
        "stopped" => "exited".to_string(),
        "" => "unknown".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_podman_stopped_onto_docker_exited() {
        assert_eq!(normalize_state("stopped"), "exited");
    }

    #[test]
    fn passes_through_known_states() {
        for s in [
            "running",
            "paused",
            "created",
            "dead",
            "restarting",
            "exited",
        ] {
            assert_eq!(normalize_state(s), s);
        }
    }

    #[test]
    fn empty_state_becomes_unknown() {
        assert_eq!(normalize_state(""), "unknown");
    }

    /// A state neither runtime documents must survive rather than break parsing.
    #[test]
    fn unknown_states_pass_through() {
        assert_eq!(normalize_state("hibernating"), "hibernating");
    }
}
