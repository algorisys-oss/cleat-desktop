//! Captures the Engine API requests Cleat actually sends.
//!
//! # Why this exists
//!
//! The activity log used to carry a method-and-path string authored by hand at
//! each call site in [`crate::runtime::audit`] — `"GET /images/json"` typed next
//! to the call it described. That is a *description*, and a description can be
//! wrong: rename an endpoint, take a bollard upgrade that changes a query
//! parameter, and the panel keeps confidently reporting the old one. The whole
//! value of the log is that a reader can trust it, so the request line has to be
//! observed rather than asserted.
//!
//! # How
//!
//! `bollard` runs a request modifier inside `build_request`, which every
//! endpoint funnels through — including the connection-upgrade path used by exec
//! attach. Hooking there sees the fully-built request: real method, real path,
//! real query string, real API version prefix.
//!
//! The modifier has no idea *which* operation it belongs to, so the operation
//! supplies the correlation instead. [`capture`] runs a future inside a
//! task-local slot and the modifier appends to whichever slot is in scope.
//! Requests are built eagerly, before any stream is returned or any task is
//! spawned, so everything an operation issues lands in that operation's slot —
//! and concurrent operations, being separate tasks, cannot see each other's.
//!
//! # What this does not cover
//!
//! Two paths do not go through bollard, and each announces itself:
//! [`crate::runtime::raw`] calls [`record`] directly, and compose is a
//! subprocess whose argv the decorator records literally.

use bollard::Docker;
use std::cell::RefCell;
use std::future::Future;

tokio::task_local! {
    /// Requests issued by the operation currently running on this task.
    static REQUESTS: RefCell<Vec<String>>;
}

/// Install the capture hook on a freshly-connected client.
///
/// Called at every construction site so there is no such thing as an
/// uninstrumented client reaching [`crate::runtime::audit`].
pub fn instrument(docker: Docker) -> Docker {
    docker.with_request_modifier(|req| {
        // `path_and_query` drops the authority, which over a unix socket is the
        // hex-encoded socket path — noise, and long enough to bury the path.
        let target = match req.uri().path_and_query() {
            Some(paq) => paq.to_string(),
            None => req.uri().to_string(),
        };
        record(format!("{} {target}", req.method()));
        req
    })
}

/// Note a request issued outside bollard.
///
/// Silently does nothing outside a [`capture`] scope: requests made during
/// connection probing are real but belong to no operation, and there is nowhere
/// honest to file them.
pub fn record(line: String) {
    let _ = REQUESTS.try_with(|slot| slot.borrow_mut().push(line));
}

/// Run `f`, returning its output alongside every request it issued, in order.
pub async fn capture<F: Future>(f: F) -> (F::Output, Vec<String>) {
    REQUESTS
        .scope(RefCell::new(Vec::new()), async move {
            let output = f.await;
            (output, REQUESTS.with(|slot| slot.take()))
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn captures_in_order() {
        let (out, requests) = capture(async {
            record("GET /v1.51/images/json".into());
            record("POST /v1.51/volumes/create".into());
            7
        })
        .await;

        assert_eq!(out, 7);
        assert_eq!(
            requests,
            ["GET /v1.51/images/json", "POST /v1.51/volumes/create"]
        );
    }

    /// An operation that issues nothing must report nothing, rather than
    /// inheriting a stale line from whatever ran before it.
    #[tokio::test]
    async fn captures_nothing_when_nothing_is_issued() {
        let (_, requests) = capture(async {}).await;
        assert!(requests.is_empty(), "unexpected: {requests:?}");
    }

    /// The probe path calls this before any operation is running.
    #[test]
    fn recording_outside_a_scope_is_a_no_op() {
        record("GET /_ping".into());
    }

    /// The log is read as "these are the calls *this* operation made", so a
    /// concurrent operation must not be able to contribute to it. Two tasks,
    /// interleaved on purpose.
    #[tokio::test]
    async fn concurrent_operations_do_not_mix() {
        let first = tokio::spawn(capture(async {
            record("GET /first".into());
            tokio::task::yield_now().await;
            record("GET /first-again".into());
        }));
        let second = tokio::spawn(capture(async {
            record("GET /second".into());
            tokio::task::yield_now().await;
            record("GET /second-again".into());
        }));

        let (_, first) = first.await.unwrap();
        let (_, second) = second.await.unwrap();
        assert_eq!(first, ["GET /first", "GET /first-again"]);
        assert_eq!(second, ["GET /second", "GET /second-again"]);
    }
}
