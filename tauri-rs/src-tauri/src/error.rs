use serde::Serialize;

/// Every failure that can reach the frontend.
///
/// The Electron backend collapsed everything into HTTP 500 regardless of what
/// the daemon actually said; `kind` preserves that distinction so the UI can
/// tell "no such container" from "daemon unreachable".
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("no container runtime is available: {0}")]
    NoRuntime(String),

    #[error("{0} is not available on this system")]
    RuntimeUnavailable(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("runtime error: {0}")]
    Engine(#[from] bollard::errors::Error),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// Stable machine-readable discriminant for the frontend.
    pub fn kind(&self) -> &'static str {
        match self {
            AppError::NoRuntime(_) => "no_runtime",
            AppError::RuntimeUnavailable(_) => "runtime_unavailable",
            AppError::NotFound(_) => "not_found",
            AppError::Conflict(_) => "conflict",
            AppError::Invalid(_) => "invalid",
            AppError::Engine(e) => match e {
                // Map the daemon's own status codes rather than flattening them.
                bollard::errors::Error::DockerResponseServerError {
                    status_code: 404, ..
                } => "not_found",
                bollard::errors::Error::DockerResponseServerError {
                    status_code: 409, ..
                } => "conflict",
                bollard::errors::Error::DockerResponseServerError {
                    status_code: 304, ..
                } => "not_modified",
                _ => "engine",
            },
            AppError::Io(_) => "io",
            AppError::Other(_) => "other",
        }
    }

    /// Unwrap the daemon's message instead of bollard's wrapper prose.
    pub fn message(&self) -> String {
        match self {
            AppError::Engine(bollard::errors::Error::DockerResponseServerError {
                message, ..
            }) => message.clone(),
            other => other.to_string(),
        }
    }
}

/// Wire shape the frontend sees when a command rejects.
#[derive(Serialize)]
pub struct SerializedError {
    pub kind: String,
    pub message: String,
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        SerializedError {
            kind: self.kind().to_string(),
            message: self.message(),
        }
        .serialize(serializer)
    }
}

pub type AppResult<T> = Result<T, AppError>;
