use std::error::Error;

use anyhow::anyhow;

use super::*;

#[test]
fn service_error_display_uses_user_facing_message() {
    assert_eq!(
        ServiceError::InvalidInput("bad timestamp".into()).to_string(),
        "bad timestamp"
    );
    assert_eq!(
        ServiceError::Busy("database worker limit reached".into()).to_string(),
        "database worker limit reached"
    );
}

#[test]
fn anyhow_errors_convert_to_internal_service_errors() {
    let err: ServiceError = anyhow!("database failed").into();

    assert!(matches!(err, ServiceError::Internal(_)));
    assert_eq!(err.to_string(), "database failed");
    assert!(Error::source(&err).is_none());
}

#[test]
fn classify_db_error_promotes_sqlite_busy_to_retryable_busy() {
    let error = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseBusy,
            extended_code: rusqlite::ffi::SQLITE_BUSY,
        },
        Some("database is locked".to_string()),
    );

    let classified = ServiceError::classify_db_error(anyhow::Error::new(error));
    assert!(matches!(classified, ServiceError::Busy(message) if message == "database_busy"));
}

#[test]
fn classify_db_error_promotes_sqlite_locked_to_retryable_busy() {
    let error = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseLocked,
            extended_code: rusqlite::ffi::SQLITE_LOCKED,
        },
        Some("database table is locked".to_string()),
    );

    let classified = ServiceError::classify_db_error(anyhow::Error::new(error));
    assert!(matches!(classified, ServiceError::Busy(message) if message == "database_busy"));
}

#[test]
fn classify_db_error_promotes_pool_timeout_to_database_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::StorageConfig::for_test(dir.path().join("classify-timeout.db"));
    let pool = crate::db::init_pool(&config).unwrap();
    let _held = pool.get().unwrap();
    let timeout = anyhow::Error::new(
        pool.get_timeout(std::time::Duration::from_millis(50))
            .expect_err("exhausted pool must time out"),
    );

    let classified = ServiceError::classify_db_error(timeout.context("read log page"));
    assert!(matches!(classified, ServiceError::DatabaseTimeout { .. }));
}

/// `DatabaseTimeout` must keep the chain it was classified from. It used to be
/// a unit variant, so the statement context and any recorded
/// connection-establishment detail were dropped at the classification boundary
/// — during an incident that left a bare "database timeout" with nothing to
/// attribute it to.
#[test]
fn database_timeout_preserves_the_source_chain() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::StorageConfig::for_test(dir.path().join("chain-timeout.db"));
    let pool = crate::db::init_pool(&config).unwrap();
    let _held = pool.get().unwrap();
    let timeout = anyhow::Error::new(
        pool.get_timeout(std::time::Duration::from_millis(50))
            .expect_err("exhausted pool must time out"),
    );

    let classified = ServiceError::classify_db_error(timeout.context("read log page"));
    let ServiceError::DatabaseTimeout { source } = &classified else {
        panic!("expected DatabaseTimeout, got {classified:?}");
    };
    let chain = format!("{source:#}");
    assert!(
        chain.contains("read log page"),
        "context lost from the chain: {chain}"
    );
    assert!(
        chain.contains("timed out waiting for connection"),
        "originating pool error lost from the chain: {chain}"
    );
    // Display is unchanged so existing log lines and the API body keep shape.
    assert_eq!(
        classified.to_string(),
        "database timeout: pool did not yield a connection in time"
    );
    assert!(std::error::Error::source(&classified).is_some());
}

#[test]
fn typed_variants_display_correctly() {
    assert_eq!(
        ServiceError::DatabaseTimeout {
            source: anyhow::anyhow!("timed out waiting for connection")
        }
        .to_string(),
        "database timeout: pool did not yield a connection in time"
    );
    assert_eq!(
        ServiceError::ConstraintViolation {
            message: "UNIQUE constraint failed: logs.id".into()
        }
        .to_string(),
        "constraint violation: UNIQUE constraint failed: logs.id"
    );
    assert_eq!(ServiceError::RowNotFound.to_string(), "row not found");
    assert_eq!(
        ServiceError::NotFound("no such host".into()).to_string(),
        "no such host"
    );
}

#[test]
fn typed_variants_match_without_downcasting() {
    let err = ServiceError::DatabaseTimeout {
        source: anyhow::anyhow!("timed out waiting for connection"),
    };
    assert!(matches!(err, ServiceError::DatabaseTimeout { .. }));

    let err = ServiceError::ConstraintViolation {
        message: "unique".into(),
    };
    assert!(matches!(err, ServiceError::ConstraintViolation { .. }));

    let err = ServiceError::RowNotFound;
    assert!(matches!(err, ServiceError::RowNotFound));
}
