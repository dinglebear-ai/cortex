use thiserror::Error;

/// Typed service-layer errors.
///
/// Prefer specific variants over `Internal` at SQLite/pool call sites. Use
/// `Internal` only for genuinely opaque errors (anyhow wrapping, etc.).
/// The `#[from] anyhow::Error` impl on `Internal` allows `?` on `anyhow`
/// result chains until all call sites are migrated to explicit `map_err`.
#[derive(Debug, Error)]
pub enum ServiceError {
    /// Caller-supplied argument was invalid.
    #[error("{0}")]
    InvalidInput(String),

    /// Resource is temporarily unavailable (e.g. SQLite BUSY / connection
    /// pool timeout). Callers may retry.
    #[error("{0}")]
    Busy(String),

    /// Requested resource does not exist.
    #[error("{0}")]
    NotFound(String),

    /// A SQLite / r2d2 pool timeout was detected. Semantic alias for `Busy`
    /// that lets callers distinguish pool starvation from other transient
    /// errors without downcasting `anyhow::Error`.
    #[error("database timeout: pool did not yield a connection in time")]
    DatabaseTimeout,

    /// A uniqueness or foreign-key constraint was violated. `message`
    /// carries the human-readable detail from the DB error.
    #[error("constraint violation: {message}")]
    ConstraintViolation { message: String },

    /// A specific row was expected to exist but was not found.
    #[error("row not found")]
    RowNotFound,

    /// Catch-all for errors that have not yet been promoted to a typed
    /// variant. Kept as `#[from] anyhow::Error` so `?` still compiles at
    /// un-migrated call sites. Gradually replace with explicit `map_err`
    /// calls that promote known error classes to the typed variants above.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub type ServiceResult<T> = Result<T, ServiceError>;

impl ServiceError {
    pub(crate) fn classify_db_error(error: anyhow::Error) -> Self {
        match error.downcast::<ServiceError>() {
            Ok(service_error) => service_error,
            Err(error) => {
                if let Some(sqlite) = error.downcast_ref::<rusqlite::Error>()
                    && is_retryable_sqlite_error(sqlite)
                {
                    return ServiceError::Busy("database_busy".to_string());
                }

                if let Some(pool_error) = error.downcast_ref::<r2d2::Error>() {
                    let message = pool_error.to_string().to_ascii_lowercase();
                    if message.contains("timed out") || message.contains("timeout") {
                        return ServiceError::DatabaseTimeout;
                    }
                }

                ServiceError::Internal(error)
            }
        }
    }
}

fn is_retryable_sqlite_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked,
                ..
            },
            _
        )
    )
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
