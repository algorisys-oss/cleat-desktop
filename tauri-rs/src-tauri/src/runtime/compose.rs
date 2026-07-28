//! Compose support.
//!
//! Compose has no Engine API — it is a CLI tool — so this is the one place the
//! app still shells out. Two rules apply here and are load-bearing:
//!
//! 1. Commands are built as argv vectors passed to `Command`, never as a
//!    string handed to a shell. The Electron backend interpolated a
//!    user-supplied `projectDir` into `exec()`, so a directory named
//!    `/tmp/x; rm -rf ~` executed. There is no shell in this path.
//! 2. The project directory is canonicalised and checked before use, so a
//!    non-existent or non-directory path fails cleanly instead of running the
//!    command in an unexpected working directory.

use crate::error::{AppError, AppResult};
use crate::model::ComposeService;
use futures_util::Stream;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Live output lines from a compose command.
pub type LineStream = Pin<Box<dyn Stream<Item = AppResult<String>> + Send>>;

/// Backstop so a wedged compose invocation cannot hold a spinner forever.
///
/// Generous on purpose: `compose up` legitimately pulls images, which can take
/// many minutes on a slow link.
const COMPOSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Validate and canonicalise a compose project directory.
pub fn resolve_project_dir(dir: &str) -> AppResult<PathBuf> {
    if dir.trim().is_empty() {
        return Err(AppError::Invalid("project directory is required".into()));
    }
    let path = PathBuf::from(dir);
    let canonical = path
        .canonicalize()
        .map_err(|e| AppError::Invalid(format!("project directory '{dir}': {e}")))?;
    if !canonical.is_dir() {
        return Err(AppError::Invalid(format!("'{dir}' is not a directory")));
    }
    Ok(canonical)
}

/// Find the compose file in a project directory, honouring both spellings.
pub fn find_compose_file(dir: &std::path::Path) -> AppResult<PathBuf> {
    for name in [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(AppError::NotFound(format!(
        "no compose file found in {}",
        dir.display()
    )))
}

/// Run `<argv> <args>` in `dir` and return stdout, or the command's stderr as an error.
pub async fn run(argv: &[String], args: &[&str], dir: &std::path::Path) -> AppResult<String> {
    let (program, leading) = argv
        .split_first()
        .ok_or_else(|| AppError::Other("empty compose command".into()))?;

    let mut cmd = Command::new(program);
    cmd.args(leading).args(args).current_dir(dir);

    let output = cmd.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::RuntimeUnavailable(format!("'{program}' is not installed or not on PATH"))
        } else {
            AppError::Io(e)
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(AppError::Other(if detail.is_empty() {
            format!("{program} exited with {}", output.status)
        } else {
            detail
        }));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Run a compose command, streaming its output line by line as it arrives.
///
/// `run()` above waits for the process to exit before returning anything, which
/// is fine for the fast `ps` but wrong for `up`/`down`: Docker waits a 10-second
/// grace period per container that ignores SIGTERM, so a multi-service stack can
/// take a minute with nothing on screen. Streaming turns that dead spinner into
/// visible progress.
///
/// Cancellation is free: `kill_on_drop` terminates the child when the consumer
/// drops the stream.
pub fn stream(argv: &[String], args: &[String], dir: &std::path::Path) -> AppResult<LineStream> {
    let (program, leading) = argv
        .split_first()
        .ok_or_else(|| AppError::Other("empty compose command".into()))?;
    let program = program.clone();

    let mut cmd = Command::new(&program);
    cmd.args(leading)
        .args(args)
        .current_dir(dir)
        // No stdin: a compose command must never block waiting on a prompt.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::RuntimeUnavailable(format!("'{program}' is not installed or not on PATH"))
        } else {
            AppError::Io(e)
        }
    })?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let (tx, rx) = tokio::sync::mpsc::channel::<AppResult<String>>(64);

    // Compose writes progress to stderr and results to stdout; merge both.
    let pump = |reader: Pin<Box<dyn tokio::io::AsyncRead + Send>>,
                tx: tokio::sync::mpsc::Sender<AppResult<String>>| {
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim_end().to_string();
                if line.is_empty() {
                    continue;
                }
                if tx.send(Ok(line)).await.is_err() {
                    break; // consumer went away
                }
            }
        })
    };

    let out_task = pump(Box::pin(stdout), tx.clone());
    let err_task = pump(Box::pin(stderr), tx.clone());

    tokio::spawn(async move {
        // Drain both pipes first so ordering stays sane, then report the exit.
        let drained = tokio::time::timeout(COMPOSE_DEADLINE, async {
            let _ = out_task.await;
            let _ = err_task.await;
            child.wait().await
        })
        .await;

        match drained {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => {
                let _ = tx
                    .send(Err(AppError::Other(format!(
                        "{program} exited with {status}"
                    ))))
                    .await;
            }
            Ok(Err(e)) => {
                let _ = tx.send(Err(AppError::Io(e))).await;
            }
            Err(_) => {
                let _ = tx
                    .send(Err(AppError::Other(format!(
                        "{program} did not finish within {} minutes; giving up",
                        COMPOSE_DEADLINE.as_secs() / 60
                    ))))
                    .await;
                // The child is killed when its handle drops (kill_on_drop).
            }
        }
    });

    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Ok(Box::pin(stream))
}

/// `compose ps --format json`, tolerant of both output shapes.
///
/// Docker Compose v2 emits one JSON object per line; some versions and
/// podman-compose emit a single JSON array. The Electron version just handed
/// the raw `docker-compose ps` table text to the UI and let it render as a
/// preformatted blob, which is why there was no per-service state to act on.
pub fn parse_ps_json(stdout: &str, fallback_project: &str) -> Vec<ComposeService> {
    let mut values: Vec<serde_json::Value> = Vec::new();

    let trimmed = stdout.trim();
    if trimmed.starts_with('[') {
        if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(trimmed) {
            values = arr;
        }
    } else {
        for line in trimmed.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                values.push(v);
            }
        }
    }

    values
        .into_iter()
        .map(|v| {
            let get = |keys: &[&str]| -> String {
                for k in keys {
                    if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
                        if !s.is_empty() {
                            return s.to_string();
                        }
                    }
                }
                String::new()
            };

            let ports_raw = get(&["Publishers", "Ports", "ports"]);
            let ports = if ports_raw.is_empty() {
                v.get("Publishers")
                    .and_then(|p| p.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| {
                                let published = p.get("PublishedPort")?.as_i64()?;
                                let target = p.get("TargetPort")?.as_i64()?;
                                if published == 0 {
                                    return None;
                                }
                                Some(format!("{published}:{target}"))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                ports_raw
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            };

            ComposeService {
                name: get(&["Service", "Name", "service"]),
                project: {
                    let p = get(&["Project", "project"]);
                    if p.is_empty() {
                        fallback_project.to_string()
                    } else {
                        p
                    }
                },
                state: get(&["State", "Status", "state"]),
                image: get(&["Image", "image"]),
                container_id: get(&["ID", "Id", "id"]),
                ports,
            }
        })
        .filter(|s| !s.name.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonl_output() {
        let out = r#"{"ID":"abc","Name":"proj-web-1","Service":"web","State":"running","Image":"nginx"}
{"ID":"def","Name":"proj-db-1","Service":"db","State":"exited","Image":"postgres"}"#;
        let svcs = parse_ps_json(out, "proj");
        assert_eq!(svcs.len(), 2);
        assert_eq!(svcs[0].name, "web");
        assert_eq!(svcs[0].state, "running");
        assert_eq!(svcs[1].image, "postgres");
    }

    #[test]
    fn parses_array_output() {
        let out = r#"[{"ID":"abc","Service":"web","State":"running","Image":"nginx","Project":"demo"}]"#;
        let svcs = parse_ps_json(out, "fallback");
        assert_eq!(svcs.len(), 1);
        assert_eq!(svcs[0].project, "demo");
    }

    #[test]
    fn falls_back_to_project_name() {
        let out = r#"{"Service":"web","State":"running"}"#;
        let svcs = parse_ps_json(out, "myproj");
        assert_eq!(svcs[0].project, "myproj");
    }

    #[test]
    fn ignores_garbage_lines() {
        let out = "not json\n{\"Service\":\"web\",\"State\":\"running\"}\n";
        assert_eq!(parse_ps_json(out, "p").len(), 1);
    }
}
