//! Dispatcher — picks a parser per (source_kind, app_name, container_name)
//! and merges its output onto the entry.
//!
//! Spec: docs/superpowers/specs/2026-05-16-enrichment-framework-design.md §4
//!
//! # Performance note: single metadata_json parse
//! `metadata_json` is parsed exactly once per `dispatch()` call at the top of
//! the function and passed as `Option<&Value>` to all helpers. Previously
//! `read_source_kind` and `read_container_name` each parsed the same JSON
//! independently (up to 4 parses per entry in the full dispatch+apply path).
//! The value lives on the stack and does not outlive the function, so no
//! `Arc<Value>` is needed.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Mutex;

use lru::LruCache;
use serde_json::Value;

use crate::db::LogBatchEntry;
use crate::enrich::output::{merge_output, record_error};
use crate::enrich::parsers::{
    AdguardParser, AutheliaParser, DockerEventParser, Fail2banParser, KernelParser, SwagParser,
};
use crate::enrich::{Parser, ParserInput, SourceKind};

const LRU_CAP: usize = 256;

/// Maps operator-renamed container names to canonical parser keys.
fn container_to_canonical(container: &str) -> &'static str {
    match container {
        "authelia" | "authelia-main" | "authelia-prod" | "authelia-master" => "authelia",
        "swag" | "swag-main" | "nginx" | "nginx-proxy" => "swag",
        "adguardhome" | "adguard" | "adguardhome-main" => "adguard",
        "fail2ban" | "fail2ban-main" => "fail2ban",
        _ => "",
    }
}

// Static singleton parser instances.
static KERNEL: KernelParser = KernelParser;
static DOCKER_EVENT: DockerEventParser = DockerEventParser;
static AUTHELIA: AutheliaParser = AutheliaParser;
static SWAG: SwagParser = SwagParser;
static ADGUARD: AdguardParser = AdguardParser;
static FAIL2BAN: Fail2banParser = Fail2banParser;

pub struct EnrichmentPipeline {
    by_name: HashMap<&'static str, &'static dyn Parser>,
    docker_event: &'static DockerEventParser,
    unknown_apps: Mutex<LruCache<String, ()>>,
}

impl EnrichmentPipeline {
    pub fn new() -> Self {
        let mut by_name: HashMap<&'static str, &'static dyn Parser> = HashMap::new();
        by_name.insert("kernel", &KERNEL);
        by_name.insert("authelia", &AUTHELIA);
        by_name.insert("swag", &SWAG);
        by_name.insert("adguard", &ADGUARD);
        by_name.insert("adguard-query", &ADGUARD); // API poller app_name
        by_name.insert("fail2ban", &FAIL2BAN);

        Self {
            by_name,
            docker_event: &DOCKER_EVENT,
            unknown_apps: Mutex::new(LruCache::new(
                NonZeroUsize::new(LRU_CAP).expect("LRU_CAP > 0"),
            )),
        }
    }

    pub fn dispatch(&self, entry: &mut LogBatchEntry) {
        // Parse metadata_json exactly once. All helpers below receive a shared
        // reference to this value — no further parsing in this dispatch path.
        let metadata: Option<Value> = entry
            .metadata_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let meta_ref = metadata.as_ref();

        let source_kind_str = extract_source_kind(meta_ref);

        // docker-event short-circuit — routed by source_kind, not app_name.
        if source_kind_str.as_deref() == Some("docker-event") {
            self.apply(entry, self.docker_event, meta_ref);
            return;
        }

        // Container-name lookup (higher priority than app_name for Docker sources).
        let container = extract_container_name(meta_ref);
        if let Some(c) = container.as_deref() {
            let canon = container_to_canonical(c);
            if !canon.is_empty()
                && let Some(&parser) = self.by_name.get(canon)
            {
                self.apply(entry, parser, meta_ref);
                return;
            }
        }

        // app_name fallback.
        let app_lower = entry.app_name.as_deref().map(|s| s.to_ascii_lowercase());
        if let Some(app) = app_lower.as_deref() {
            if let Some(&parser) = self.by_name.get(app) {
                self.apply(entry, parser, meta_ref);
                return;
            }
            // Debug-log unknown app_names once per unique value.
            if let Ok(mut lru) = self.unknown_apps.lock()
                && lru.put(app.to_string(), ()).is_none()
            {
                tracing::debug!(app_name = app, "enrich: no parser registered for app_name");
            }
        }
    }

    fn apply(
        &self,
        entry: &mut LogBatchEntry,
        parser: &'static dyn Parser,
        meta_ref: Option<&Value>,
    ) {
        // Extract owned copies of string fields before taking a mutable borrow
        // of entry in merge_output/record_error so the borrow checker is
        // satisfied and the parser receives consistent values (not stale data
        // after mutation).
        //
        // source_kind and container_name are derived from the already-parsed
        // `meta_ref` — no second JSON parse here.
        let source_kind = to_source_kind(extract_source_kind(meta_ref).as_deref());
        let container = extract_container_name(meta_ref);
        let app_name = entry.app_name.clone();
        let message = entry.message.clone();
        let raw = entry.raw.clone();
        let severity = entry.severity.clone();
        let input = ParserInput {
            app_name: app_name.as_deref(),
            container_name: container.as_deref(),
            message: &message,
            raw: &raw,
            source_kind,
            severity: &severity,
        };
        match parser.parse(input) {
            Ok(out) => merge_output(entry, parser.namespace(), out),
            Err(e) => record_error(entry, parser.name(), &e.to_string()),
        }
    }
}

impl Default for EnrichmentPipeline {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the `source_kind` string from an already-parsed metadata value.
/// Returns `None` if the metadata is absent, the key is missing, or the value
/// is not a string.
fn extract_source_kind(meta: Option<&Value>) -> Option<String> {
    meta?.get("source_kind")?.as_str().map(str::to_string)
}

/// Map a raw `source_kind` string onto the [`SourceKind`] enum.
fn to_source_kind(raw: Option<&str>) -> SourceKind {
    match raw {
        Some("syslog-udp") => SourceKind::SyslogUdp,
        Some("syslog-tcp") => SourceKind::SyslogTcp,
        Some("docker-stream") => SourceKind::DockerStream,
        Some("docker-event") => SourceKind::DockerEvent,
        Some("otlp") => SourceKind::Otlp,
        Some("adguard-api") => SourceKind::AdguardApi,
        Some("unifi-api") => SourceKind::UnifiApi,
        Some("agent") => SourceKind::Agent,
        Some("shell-history") => SourceKind::ShellHistory,
        Some("agent-command") => SourceKind::AgentCommand,
        Some("file-tail") => SourceKind::FileTail,
        _ => SourceKind::SyslogTcp,
    }
}

/// Extract the `container_name` string from an already-parsed metadata value.
///
/// `docker_ingest/parser.rs` stamps `container_name` at the root of
/// `metadata_json` (not under a "docker" sub-object), so look for it directly.
fn extract_container_name(meta: Option<&Value>) -> Option<String> {
    meta?.get("container_name")?.as_str().map(str::to_string)
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod dispatch_tests;
