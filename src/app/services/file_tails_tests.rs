use std::sync::Arc;

use crate::app::{FileTailAddRequest, FileTailOp, FileTailRequest, ServiceError};
use crate::config::StorageConfig;
use crate::db::init_pool;
use crate::filetail::{FileTailRegistry, FileTailSource};

use super::CortexService;

fn failing_service() -> (CortexService, Arc<FileTailRegistry>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(temp.path().join("file-tail-rollback.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    let registry = Arc::new(FileTailRegistry::new(temp.path().join("file-tails.json")));
    let service = CortexService::new(pool, storage).with_file_tail_control(
        Arc::clone(&registry),
        Arc::new(|| anyhow::bail!("supervisor unavailable")),
        Arc::new(Vec::new),
    );
    (service, registry, temp)
}

fn add_request(path: &std::path::Path, id: &str) -> FileTailRequest {
    FileTailRequest::add(FileTailAddRequest {
        id: id.into(),
        path: path.to_string_lossy().into_owned(),
        tag: id.into(),
        host: Some("edgehost".into()),
        facility: None,
        severity: None,
        start_at_end: None,
    })
}

fn seed(registry: &FileTailRegistry, path: &std::path::Path, enabled: bool) -> FileTailSource {
    let mut source = FileTailSource::from_add(
        FileTailAddRequest {
            id: "swag-access".into(),
            path: path.to_string_lossy().into_owned(),
            tag: "swag-access".into(),
            host: Some("edgehost".into()),
            facility: None,
            severity: None,
            start_at_end: None,
        },
        "2026-07-16T20:00:00Z",
    )
    .unwrap();
    source.enabled = enabled;
    registry.upsert(source.clone()).unwrap();
    source
}

fn assert_rolled_back(error: ServiceError) {
    assert!(
        error.to_string().contains("registry mutation rolled back"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn add_restores_empty_registry_when_reconcile_fails() {
    let (service, registry, temp) = failing_service();
    let path = temp.path().join("access.log");
    std::fs::write(&path, "seed\n").unwrap();

    let error = service
        .file_tails(add_request(&path, "swag-access"))
        .await
        .unwrap_err();

    assert_rolled_back(error);
    assert!(registry.list().unwrap().is_empty());
}

#[tokio::test]
async fn remove_restores_source_when_reconcile_fails() {
    let (service, registry, temp) = failing_service();
    let path = temp.path().join("access.log");
    std::fs::write(&path, "seed\n").unwrap();
    let original = seed(&registry, &path, true);

    let error = service
        .file_tails(FileTailRequest::id_op(
            FileTailOp::Remove,
            original.id.clone(),
        ))
        .await
        .unwrap_err();

    assert_rolled_back(error);
    assert_eq!(registry.list().unwrap(), vec![original]);
}

#[tokio::test]
async fn enable_restores_disabled_source_when_reconcile_fails() {
    let (service, registry, temp) = failing_service();
    let path = temp.path().join("access.log");
    std::fs::write(&path, "seed\n").unwrap();
    let original = seed(&registry, &path, false);

    let error = service
        .file_tails(FileTailRequest::id_op(
            FileTailOp::Enable,
            original.id.clone(),
        ))
        .await
        .unwrap_err();

    assert_rolled_back(error);
    assert_eq!(registry.list().unwrap(), vec![original]);
}

#[tokio::test]
async fn disable_restores_enabled_source_when_reconcile_fails() {
    let (service, registry, temp) = failing_service();
    let path = temp.path().join("access.log");
    std::fs::write(&path, "seed\n").unwrap();
    let original = seed(&registry, &path, true);

    let error = service
        .file_tails(FileTailRequest::id_op(
            FileTailOp::Disable,
            original.id.clone(),
        ))
        .await
        .unwrap_err();

    assert_rolled_back(error);
    assert_eq!(registry.list().unwrap(), vec![original]);
}
