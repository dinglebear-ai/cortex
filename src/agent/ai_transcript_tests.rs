use super::*;
use std::io::Write;

fn write_file(path: &Path, content: &str) {
    let mut file = fs::File::create(path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
}

fn accepted_receipt_response(request: &wiremock::Request) -> wiremock::ResponseTemplate {
    let value: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    let receipts: Vec<_> = value["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            serde_json::json!({
                "source_record_id": record["envelope"]["source_record_id"],
                "disposition": "accepted",
            })
        })
        .collect();
    wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "accepted": receipts.len(),
        "receipts": receipts,
    }))
}

#[test]
fn collect_files_finds_supported_and_skips_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude/projects/foo");
    fs::create_dir_all(&claude_dir).unwrap();
    write_file(&claude_dir.join("session.jsonl"), "{}\n");
    write_file(&claude_dir.join("readme.txt"), "not a transcript\n");

    let mut out = Vec::new();
    collect_files(dir.path(), &mut out);
    assert_eq!(out.len(), 1);
    assert!(out[0].ends_with("session.jsonl"));
}

#[test]
fn collect_files_skips_build_artifact_directories() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join(".codex/worktrees/session-id/lab");
    let target = project.join("target/debug/.fingerprint/package");
    let node_modules = project.join("node_modules/package");
    let cache = project.join(".cache/cargo/release/deps/rustc123");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(&node_modules).unwrap();
    fs::create_dir_all(&cache).unwrap();
    write_file(&project.join("rollout-session.jsonl"), "{}\n");
    write_file(&target.join("not-a-transcript.jsonl"), "{}\n");
    write_file(&node_modules.join("also-not-a-transcript.jsonl"), "{}\n");
    write_file(&cache.join("transient-not-a-transcript.jsonl"), "{}\n");

    let mut out = Vec::new();
    collect_files(dir.path(), &mut out);

    assert_eq!(out.len(), 1);
    assert!(out[0].ends_with("rollout-session.jsonl"));
}

#[test]
fn configured_mounted_roots_keep_provider_layout_classification() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        forward_source_kind(&root.path().join(".claude/projects/project/session.jsonl")),
        scanner::SourceKind::ClaudeProject
    );
    assert_eq!(
        forward_source_kind(&root.path().join(".codex/sessions/2026/09/session.jsonl")),
        scanner::SourceKind::CodexSession
    );
    assert_eq!(
        forward_source_kind(&root.path().join(".gemini/tmp/session/chats/session-1.json")),
        scanner::SourceKind::GeminiSession
    );
}

#[test]
fn forwarded_evidence_capabilities_follow_the_provider_registry() {
    let claude = capability_coverage(scanner::SourceKind::ClaudeProject);
    assert_eq!(claude.transcript, EvidenceCoverage::Observed);
    assert_eq!(claude.mcp_events, EvidenceCoverage::Partial);
    assert_eq!(claude.skill_events, EvidenceCoverage::Partial);
    assert_eq!(claude.hook_events, EvidenceCoverage::Partial);

    let codex = capability_coverage(scanner::SourceKind::CodexSession);
    assert_eq!(codex.transcript, EvidenceCoverage::Observed);
    assert_eq!(codex.mcp_events, EvidenceCoverage::Partial);
    assert_eq!(codex.skill_events, EvidenceCoverage::Partial);
    assert_eq!(codex.hook_events, EvidenceCoverage::NotObserved);

    let gemini = capability_coverage(scanner::SourceKind::GeminiSession);
    assert_eq!(gemini.transcript, EvidenceCoverage::Observed);
    assert_eq!(gemini.mcp_events, EvidenceCoverage::NotObserved);
    assert_eq!(gemini.skill_events, EvidenceCoverage::NotObserved);
    assert_eq!(gemini.hook_events, EvidenceCoverage::NotObserved);

    // Explicit files remain useful, bounded transcript inputs, but have no
    // provider descriptor and therefore cannot claim extracted event lanes.
    let explicit = capability_coverage(scanner::SourceKind::ExplicitFile);
    assert_eq!(explicit.transcript, EvidenceCoverage::Partial);
    assert_eq!(explicit.mcp_events, EvidenceCoverage::NotObserved);
    assert_eq!(explicit.skill_events, EvidenceCoverage::NotObserved);
    assert_eq!(explicit.hook_events, EvidenceCoverage::NotObserved);
}

#[cfg(unix)]
#[test]
fn collect_files_never_follows_symlinked_transcript() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let transcript = outside.path().join("session.jsonl");
    write_file(&transcript, "{}\n");
    symlink(&transcript, root.path().join("session.jsonl")).unwrap();

    let mut files = Vec::new();
    collect_files(root.path(), &mut files);
    assert!(files.is_empty());
}

#[test]
fn read_new_lines_returns_only_lines_past_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    write_file(&path, "line0\nline1\nline2\n");

    let (lines, total) = read_new_lines(&path, 1, 500).unwrap();
    assert_eq!(total, 3);
    assert_eq!(
        lines,
        vec![(1, "line1".to_string()), (2, "line2".to_string())]
    );
}

#[test]
fn read_new_lines_respects_limit_and_reports_checkpoint_at_cutoff_not_eof() {
    // Regression: the checkpoint returned must reflect how far the limited
    // read actually got, not the file's true EOF — otherwise lines past the
    // limit are silently skipped forever on the next call.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    write_file(&path, "line0\nline1\nline2\nline3\nline4\n");

    let (lines, checkpoint) = read_new_lines(&path, 0, 2).unwrap();
    assert_eq!(
        lines,
        vec![(0, "line0".to_string()), (1, "line1".to_string())]
    );
    assert_eq!(
        checkpoint, 2,
        "checkpoint must stop at the limit, not report EOF (5)"
    );

    let (lines, checkpoint) = read_new_lines(&path, checkpoint, 2).unwrap();
    assert_eq!(
        lines,
        vec![(2, "line2".to_string()), (3, "line3".to_string())]
    );
    assert_eq!(checkpoint, 4);
}

#[test]
fn checkpoint_round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let checkpoint_path = dir.path().join("checkpoint.json");
    let mut checkpoint = Checkpoint::default();
    checkpoint.files.insert("/tmp/foo.jsonl".to_string(), 42);
    checkpoint
        .fingerprints
        .insert("/tmp/foo.jsonl".to_string(), "sha256:prefix".to_string());
    checkpoint.gemini_parse_failures.insert(
        "/tmp/bad-gemini.json".to_string(),
        GeminiParseFailure {
            fingerprint: 99,
            last_warned: Instant::now(),
        },
    );
    save_checkpoint(&checkpoint_path, &checkpoint).unwrap();

    let loaded = load_checkpoint(&checkpoint_path);
    assert_eq!(loaded.files.get("/tmp/foo.jsonl"), Some(&42));
    assert_eq!(
        loaded
            .fingerprints
            .get("/tmp/foo.jsonl")
            .map(String::as_str),
        Some("sha256:prefix")
    );
    assert!(
        loaded.gemini_parse_failures.is_empty(),
        "parse-warning suppression is process-local and must not be persisted"
    );
}

#[test]
fn envelope_identity_survives_an_archive_move_when_native_session_is_available() {
    let root = tempfile::tempdir().unwrap();
    let active = root.path().join(".claude/projects/project/session.jsonl");
    fs::create_dir_all(active.parent().unwrap()).unwrap();
    write_file(&active, "same record\n");
    let config = AiTranscriptForwardConfig {
        roots: vec![root.path().to_path_buf()],
        target: "http://unused".to_string(),
        token: None,
        hostname: "host-a".to_string(),
        checkpoint_path: root.path().join("checkpoint.json"),
        poll_interval: Duration::from_secs(1),
    };
    let before = transcript_record(
        &config,
        &active,
        scanner::SourceKind::ClaudeProject,
        TranscriptRecordDetails {
            revision: "line:0:same record".to_string(),
            timestamp: None,
            ai_project: None,
            ai_session_id: Some("native-session".to_string()),
            event_kind: Some("user".to_string()),
            message: "same record".to_string(),
        },
    );
    let archived = root
        .path()
        .join(".claude/projects/project/archive/session.jsonl");
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    fs::rename(&active, &archived).unwrap();
    let after = transcript_record(
        &config,
        &archived,
        scanner::SourceKind::ClaudeProject,
        TranscriptRecordDetails {
            revision: "line:0:same record".to_string(),
            timestamp: None,
            ai_project: None,
            ai_session_id: Some("native-session".to_string()),
            event_kind: Some("user".to_string()),
            message: "same record".to_string(),
        },
    );
    assert_eq!(
        before.envelope.source_record_id, after.envelope.source_record_id,
        "archive moves must replay to the same receipt identity"
    );
    assert_ne!(
        before.envelope.source.locator, after.envelope.source.locator,
        "safe locator records that the mutable location changed"
    );
}

#[test]
fn gemini_parse_failure_warns_once_per_content_revision() {
    let mut checkpoint = Checkpoint::default();
    let key = "/tmp/bad-gemini.json";
    let now = Instant::now();

    assert!(should_warn_gemini_parse_failure(
        &mut checkpoint,
        key,
        "{not-json",
        now
    ));
    assert!(
        !should_warn_gemini_parse_failure(&mut checkpoint, key, "{not-json", now),
        "unchanged malformed content must not warn every poll"
    );
    assert!(
        should_warn_gemini_parse_failure(&mut checkpoint, key, "{still-not-json", now),
        "a changed malformed revision should warn once again"
    );
}

/// The failure mode content-only suppression creates: a transcript that goes
/// malformed and then stops changing would otherwise warn once and go silent
/// for the process lifetime while its data is never forwarded.
#[test]
fn gemini_parse_failure_rewarns_after_the_interval_even_when_content_is_unchanged() {
    let mut checkpoint = Checkpoint::default();
    let key = "/tmp/stuck-gemini.json";
    let start = Instant::now();

    assert!(should_warn_gemini_parse_failure(
        &mut checkpoint,
        key,
        "{not-json",
        start
    ));

    let just_before = start + GEMINI_REWARN_INTERVAL - Duration::from_secs(1);
    assert!(
        !should_warn_gemini_parse_failure(&mut checkpoint, key, "{not-json", just_before),
        "must stay quiet until the re-warn interval elapses"
    );

    let after = start + GEMINI_REWARN_INTERVAL;
    assert!(
        should_warn_gemini_parse_failure(&mut checkpoint, key, "{not-json", after),
        "a persistently malformed transcript must not go dark forever"
    );
    assert!(
        !should_warn_gemini_parse_failure(&mut checkpoint, key, "{not-json", after),
        "the re-warn must reset the clock, not latch on"
    );
}

#[test]
fn gemini_parse_failures_are_evicted_for_files_that_no_longer_exist() {
    let mut checkpoint = Checkpoint::default();
    let now = Instant::now();
    should_warn_gemini_parse_failure(&mut checkpoint, "/tmp/gone.json", "{bad", now);
    should_warn_gemini_parse_failure(&mut checkpoint, "/tmp/still-here.json", "{bad", now);

    let present: HashSet<String> = ["/tmp/still-here.json".to_string()].into_iter().collect();
    evict_missing_gemini_failures(&mut checkpoint, &present);

    assert!(
        !checkpoint
            .gemini_parse_failures
            .contains_key("/tmp/gone.json")
    );
    assert!(
        checkpoint
            .gemini_parse_failures
            .contains_key("/tmp/still-here.json")
    );
}

#[tokio::test]
async fn scan_and_forward_sends_new_lines_and_advances_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude/projects/foo");
    fs::create_dir_all(&claude_dir).unwrap();
    let transcript_path = claude_dir.join("session.jsonl");
    write_file(
        &transcript_path,
        &format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "timestamp": "2026-07-09T00:00:00Z",
                "sessionId": "sess-1",
                "message": {"role": "user", "content": "hello world"}
            })
        ),
    );

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/ai-transcripts"))
        .respond_with(accepted_receipt_response)
        .expect(1)
        .mount(&server)
        .await;

    let config = AiTranscriptForwardConfig {
        roots: vec![dir.path().to_path_buf()],
        target: server.uri(),
        token: Some("test-token".to_string()),
        hostname: "test-host".to_string(),
        checkpoint_path: dir.path().join("checkpoint.json"),
        poll_interval: Duration::from_secs(15),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();
    let sent = scan_and_forward(&config, &client, &mut checkpoint)
        .await
        .unwrap();
    assert_eq!(sent, 1);
    assert_eq!(
        checkpoint
            .files
            .get(&transcript_path.to_string_lossy().to_string()),
        Some(&1)
    );

    // Second scan with no new lines should send nothing.
    let sent_again = scan_and_forward(&config, &client, &mut checkpoint)
        .await
        .unwrap();
    assert_eq!(sent_again, 0);
}

#[tokio::test]
async fn scan_and_forward_retries_after_lost_or_incomplete_receipt_response() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude/projects/foo");
    fs::create_dir_all(&claude_dir).unwrap();
    let transcript_path = claude_dir.join("session.jsonl");
    write_file(
        &transcript_path,
        &format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "timestamp": "2026-07-09T00:00:00Z",
                "sessionId": "sess-1",
                "message": {"role": "user", "content": "retry me"}
            })
        ),
    );
    let server = wiremock::MockServer::start().await;
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_clone = calls.clone();
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/ai-transcripts"))
        .respond_with(move |request: &wiremock::Request| {
            if calls_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                // Model the canonical commit succeeding but its receipt being
                // lost/truncated on the return path. The sender must not move
                // its cursor without exact receipt IDs.
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"accepted": 1, "receipts": []}))
            } else {
                accepted_receipt_response(request)
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let config = AiTranscriptForwardConfig {
        roots: vec![dir.path().to_path_buf()],
        target: server.uri(),
        token: None,
        hostname: "test-host".to_string(),
        checkpoint_path: dir.path().join("checkpoint.json"),
        poll_interval: Duration::from_secs(15),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();
    assert!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .is_err()
    );
    assert!(
        checkpoint.files.is_empty(),
        "no receipt means no checkpoint"
    );
    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        checkpoint
            .files
            .get(&transcript_path.to_string_lossy().to_string()),
        Some(&1)
    );
}

#[tokio::test]
async fn scan_and_forward_replays_a_rewritten_jsonl_source_from_zero() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude/projects/foo");
    fs::create_dir_all(&claude_dir).unwrap();
    let transcript_path = claude_dir.join("session.jsonl");
    let line = |content: &str| {
        format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "timestamp": "2026-07-09T00:00:00Z",
                "sessionId": "sess-1",
                "message": {"role": "user", "content": content}
            })
        )
    };
    write_file(&transcript_path, &line("before rewrite"));

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/ai-transcripts"))
        .respond_with(accepted_receipt_response)
        .expect(2)
        .mount(&server)
        .await;
    let config = AiTranscriptForwardConfig {
        roots: vec![dir.path().to_path_buf()],
        target: server.uri(),
        token: None,
        hostname: "test-host".to_string(),
        checkpoint_path: dir.path().join("checkpoint.json"),
        poll_interval: Duration::from_secs(15),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();
    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        checkpoint
            .files
            .get(&transcript_path.to_string_lossy().to_string()),
        Some(&1)
    );

    // Same line count but a different prefix: never treat this as an append.
    write_file(&transcript_path, &line("after rewrite"));
    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        checkpoint
            .files
            .get(&transcript_path.to_string_lossy().to_string()),
        Some(&1)
    );
}

#[tokio::test]
async fn scan_and_forward_preserves_codex_prefix_metadata_after_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let codex_dir = dir.path().join(".codex/sessions/2026/07/12");
    fs::create_dir_all(&codex_dir).unwrap();
    let transcript_path = codex_dir.join("rollout-2026-07-12T22-31-12-codex-sess-1.jsonl");
    write_file(
        &transcript_path,
        &format!(
            "{}\n{}\n",
            serde_json::json!({
                "timestamp": "2026-07-09T00:00:00Z",
                "type": "session_meta",
                "payload": {
                    "id": "codex-sess-1",
                    "cwd": "/home/jmagar/workspace/cortex"
                }
            }),
            serde_json::json!({
                "timestamp": "2026-07-09T00:00:01Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "content": "hello from codex"
                }
            })
        ),
    );

    let server = wiremock::MockServer::start().await;
    let received = std::sync::Arc::new(std::sync::Mutex::new(None));
    let received_clone = received.clone();
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/ai-transcripts"))
        .respond_with(move |req: &wiremock::Request| {
            *received_clone.lock().unwrap() = Some(req.body.clone());
            accepted_receipt_response(req)
        })
        .expect(1)
        .mount(&server)
        .await;

    let config = AiTranscriptForwardConfig {
        roots: vec![dir.path().to_path_buf()],
        target: server.uri(),
        token: None,
        hostname: "test-host".to_string(),
        checkpoint_path: dir.path().join("checkpoint.json"),
        poll_interval: Duration::from_secs(15),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();
    checkpoint
        .files
        .insert(transcript_path.to_string_lossy().to_string(), 1);

    let sent = scan_and_forward(&config, &client, &mut checkpoint)
        .await
        .unwrap();
    assert_eq!(sent, 1);

    let body = received.lock().unwrap().take().unwrap();
    let request: AiTranscriptIngestRequest = serde_json::from_slice(&body).unwrap();
    assert_eq!(request.records.len(), 1);
    let envelope = &request.records[0].envelope;
    assert_eq!(envelope.source.provider, "codex");
    assert!(
        envelope
            .ai_project
            .as_deref()
            .is_some_and(|value| value.starts_with("project:sha256:"))
    );
    assert!(
        envelope
            .ai_session_id
            .as_deref()
            .is_some_and(|value| value.starts_with("session:sha256:"))
    );
}

#[tokio::test]
async fn scan_and_forward_scrubs_credentials_before_sending() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude/projects/foo");
    fs::create_dir_all(&claude_dir).unwrap();
    write_file(
        &claude_dir.join("session.jsonl"),
        &format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "timestamp": "2026-07-09T00:00:00Z",
                "sessionId": "sess-1",
                "message": {"role": "user", "content": "export OPENAI_API_KEY=sk-proj-super-secret-value-long-enough-to-match"}
            })
        ),
    );

    let server = wiremock::MockServer::start().await;
    let received = std::sync::Arc::new(std::sync::Mutex::new(None));
    let received_clone = received.clone();
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/ai-transcripts"))
        .respond_with(move |req: &wiremock::Request| {
            *received_clone.lock().unwrap() = Some(req.body.clone());
            accepted_receipt_response(req)
        })
        .expect(1)
        .mount(&server)
        .await;

    let config = AiTranscriptForwardConfig {
        roots: vec![dir.path().to_path_buf()],
        target: server.uri(),
        token: None,
        hostname: "test-host".to_string(),
        checkpoint_path: dir.path().join("checkpoint.json"),
        poll_interval: Duration::from_secs(15),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();
    scan_and_forward(&config, &client, &mut checkpoint)
        .await
        .unwrap();

    let body = received.lock().unwrap().take().unwrap();
    let body_str = String::from_utf8(body).unwrap();
    assert!(
        !body_str.contains("sk-proj-super-secret-value-long-enough-to-match"),
        "raw API key must not reach the network: {body_str}"
    );
    assert!(body_str.contains("REDACTED"), "got: {body_str}");
}

#[tokio::test]
async fn scan_and_forward_clears_gemini_parse_failure_after_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let gemini_dir = dir.path().join(".gemini/tmp/abc123/chats");
    fs::create_dir_all(&gemini_dir).unwrap();
    let session_path = gemini_dir.join("session-1.json");
    write_file(&session_path, "{not-json");

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/ai-transcripts"))
        .respond_with(accepted_receipt_response)
        .expect(1)
        .mount(&server)
        .await;

    let config = AiTranscriptForwardConfig {
        roots: vec![dir.path().to_path_buf()],
        target: server.uri(),
        token: None,
        hostname: "test-host".to_string(),
        checkpoint_path: dir.path().join("checkpoint.json"),
        poll_interval: Duration::from_secs(15),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();
    let key = session_path.to_string_lossy().to_string();

    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        0
    );
    assert!(checkpoint.gemini_parse_failures.contains_key(&key));

    write_file(
        &session_path,
        &serde_json::json!({
            "sessionId": "gemini-sess-1",
            "cwd": "/home/jmagar/workspace/cortex",
            "messages": [
                {"id": "m1", "timestamp": "2026-07-09T00:00:00Z", "content": "recovered"},
            ]
        })
        .to_string(),
    );

    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        1
    );
    assert!(!checkpoint.gemini_parse_failures.contains_key(&key));
}

#[tokio::test]
async fn scan_and_forward_handles_gemini_whole_file_session_with_record_index_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let gemini_dir = dir.path().join(".gemini/tmp/abc123/chats");
    fs::create_dir_all(&gemini_dir).unwrap();
    let session_path = gemini_dir.join("session-1.json");
    write_file(
        &session_path,
        &serde_json::json!({
            "sessionId": "gemini-sess-1",
            "cwd": "/home/jmagar/workspace/cortex",
            "messages": [
                {"id": "m1", "timestamp": "2026-07-09T00:00:00Z", "content": "first message"},
            ]
        })
        .to_string(),
    );

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/ai-transcripts"))
        .respond_with(accepted_receipt_response)
        .mount(&server)
        .await;

    let config = AiTranscriptForwardConfig {
        roots: vec![dir.path().to_path_buf()],
        target: server.uri(),
        token: None,
        hostname: "test-host".to_string(),
        checkpoint_path: dir.path().join("checkpoint.json"),
        poll_interval: Duration::from_secs(15),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();

    let sent = scan_and_forward(&config, &client, &mut checkpoint)
        .await
        .unwrap();
    assert_eq!(sent, 1);
    assert_eq!(
        checkpoint
            .files
            .get(&session_path.to_string_lossy().to_string()),
        Some(&1),
        "gemini checkpoint tracks a record index, not a byte offset"
    );

    // No new messages yet: re-scanning must send nothing.
    let sent_again = scan_and_forward(&config, &client, &mut checkpoint)
        .await
        .unwrap();
    assert_eq!(sent_again, 0);

    // Gemini rewrites the whole file with the new message appended —
    // only the new one (past the checkpoint) should forward next cycle.
    write_file(
        &session_path,
        &serde_json::json!({
            "sessionId": "gemini-sess-1",
            "cwd": "/home/jmagar/workspace/cortex",
            "messages": [
                {"id": "m1", "timestamp": "2026-07-09T00:00:00Z", "content": "first message"},
                {"id": "m2", "timestamp": "2026-07-09T00:01:00Z", "content": "second message"},
            ]
        })
        .to_string(),
    );
    let sent_third = scan_and_forward(&config, &client, &mut checkpoint)
        .await
        .unwrap();
    assert_eq!(sent_third, 1);
    assert_eq!(
        checkpoint
            .files
            .get(&session_path.to_string_lossy().to_string()),
        Some(&2)
    );
}

#[tokio::test]
async fn scan_and_forward_replays_a_rewritten_gemini_acknowledged_prefix() {
    use std::sync::{Arc, Mutex};

    let dir = tempfile::tempdir().unwrap();
    let gemini_dir = dir.path().join(".gemini/tmp/abc123/chats");
    fs::create_dir_all(&gemini_dir).unwrap();
    let session_path = gemini_dir.join("session-1.json");
    let session = |first: &str, include_second: bool| {
        let mut messages = vec![serde_json::json!({
            "id": "m1",
            "timestamp": "2026-07-09T00:00:00Z",
            "content": first,
        })];
        if include_second {
            messages.push(serde_json::json!({
                "id": "m2",
                "timestamp": "2026-07-09T00:01:00Z",
                "content": "second message",
            }));
        }
        serde_json::json!({
            "sessionId": "gemini-sess-1",
            "cwd": "/home/jmagar/workspace/cortex",
            "messages": messages,
        })
        .to_string()
    };
    write_file(&session_path, &session("first message", false));

    let server = wiremock::MockServer::start().await;
    let seen_ids = Arc::new(Mutex::new(HashSet::<String>::new()));
    let batches = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let response_seen_ids = seen_ids.clone();
    let response_batches = batches.clone();
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/ai-transcripts"))
        .respond_with(move |request: &wiremock::Request| {
            let value: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let ids: Vec<String> = value["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|record| record["envelope"]["source_record_id"].as_str().unwrap().to_owned())
                .collect();
            response_batches.lock().unwrap().push(ids.clone());
            let mut seen = response_seen_ids.lock().unwrap();
            let receipts: Vec<_> = ids
                .into_iter()
                .map(|source_record_id| {
                    let disposition = if seen.insert(source_record_id.clone()) {
                        "accepted"
                    } else {
                        "duplicate"
                    };
                    serde_json::json!({"source_record_id": source_record_id, "disposition": disposition})
                })
                .collect();
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "accepted": receipts.len(),
                "receipts": receipts,
            }))
        })
        .expect(3)
        .mount(&server)
        .await;

    let config = AiTranscriptForwardConfig {
        roots: vec![dir.path().to_path_buf()],
        target: server.uri(),
        token: None,
        hostname: "test-host".to_string(),
        checkpoint_path: dir.path().join("checkpoint.json"),
        poll_interval: Duration::from_secs(15),
    };
    let client = reqwest::Client::new();
    let mut checkpoint = Checkpoint::default();

    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        1
    );

    // A normal whole-file append keeps the acknowledged semantic prefix and
    // forwards only the newly added record.
    write_file(&session_path, &session("first message", true));
    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        checkpoint
            .files
            .get(&session_path.to_string_lossy().to_string()),
        Some(&2)
    );

    // Rewriting an already acknowledged record resets to zero. The unchanged
    // second record receives a duplicate receipt while the changed first
    // record is accepted; both exact IDs must let the checkpoint advance.
    write_file(&session_path, &session("rewritten first message", true));
    assert_eq!(
        scan_and_forward(&config, &client, &mut checkpoint)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        checkpoint
            .files
            .get(&session_path.to_string_lossy().to_string()),
        Some(&2)
    );

    let batches = batches.lock().unwrap();
    assert_eq!(
        batches.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![1, 1, 2]
    );
    assert_ne!(
        batches[0][0], batches[2][0],
        "rewritten first record has a new identity"
    );
    assert_eq!(
        batches[1][0], batches[2][1],
        "unchanged second record is replayed idempotently"
    );
}
