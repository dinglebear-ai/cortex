use super::super::color::{cyan, muted, primary, warn};
use super::super::output::common::print_json;
use super::super::sparkline::sparkline;
use super::http_or_cancel;

use anyhow::{Result, bail};
use cortex::app::{
    AckErrorRequest, IngestRateRequest, ListSourceIpsRequest, PatternsRequest, TimelineRequest,
    UnackErrorRequest, UnaddressedErrorsRequest,
};

use super::super::{
    CliMode, IngestRateArgs, NotifyRecentArgs, NotifyTestArgs, PatternsArgs, SigAckArgs,
    SigListArgs, SigUnackArgs, SourceIpsArgs, TimelineArgs,
};

// ─── Surface parity (hosts sources, timeline, patterns, ingest-rate, sig, notify) ─

impl SourceIpsArgs {
    pub(crate) fn into_request(self) -> ListSourceIpsRequest {
        ListSourceIpsRequest {
            limit: self.limit,
            offset: self.offset,
        }
    }
}

impl TimelineArgs {
    pub(crate) fn into_request(self) -> TimelineRequest {
        // Default lookback is centralized in `CortexService::timeline` (bead dyqw):
        // it applies a bucket-sized window only when neither `since` nor `until` is set.
        // Both CLI modes reach that service (local directly, HTTP via the server),
        // so we pass `since`/`until` through verbatim — no per-binary duplication.
        TimelineRequest {
            bucket: self.bucket,
            group_by: self.group_by,
            since: self.since,
            until: self.until,
            host: self.host,
            app: self.app,
            severity_min: self.severity_min,
        }
    }
}

impl PatternsArgs {
    pub(crate) fn into_request(self) -> PatternsRequest {
        PatternsRequest {
            since: self.since,
            until: self.until,
            host: self.host,
            app: self.app,
            severity_min: self.severity_min,
            scan_limit: self.scan_limit,
            top_n: self.top_n,
        }
    }
}

impl IngestRateArgs {
    pub(crate) fn into_request(self) -> IngestRateRequest {
        IngestRateRequest {
            by_host: if self.by_host { Some(true) } else { None },
        }
    }
}

impl SigListArgs {
    pub(crate) fn into_request(self) -> UnaddressedErrorsRequest {
        UnaddressedErrorsRequest {
            limit: self.limit,
            include_acknowledged: Some(self.include_acknowledged),
        }
    }
}

impl SigAckArgs {
    pub(crate) fn into_request(self) -> AckErrorRequest {
        AckErrorRequest {
            signature_hash: self.signature_hash,
            notes: self.notes,
        }
    }
}

impl SigUnackArgs {
    pub(crate) fn into_request(self) -> UnackErrorRequest {
        UnackErrorRequest {
            signature_hash: self.signature_hash,
            reason: self.reason,
        }
    }
}

pub(crate) async fn run_source_ips(mode: &CliMode, args: SourceIpsArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.hosts().source_ips(req).await?,
        CliMode::Http(client) => http_or_cancel(client.source_ips(&req)).await?,
    };
    if json {
        return print_json(&response);
    }
    println!(
        "{} source IP(s) (total {}):",
        cyan(&response.source_ips.len().to_string()),
        cyan(&response.total.to_string())
    );
    for ip in &response.source_ips {
        println!(
            "  {:<20} logs={} hosts={} last_seen={}",
            primary(&ip.source_ip),
            cyan(&ip.log_count.to_string()),
            cyan(&ip.host_count.to_string()),
            muted(&ip.last_seen)
        );
    }
    Ok(())
}

pub(crate) async fn run_timeline(mode: &CliMode, args: TimelineArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.stats().timeline(req).await?,
        CliMode::Http(client) => http_or_cancel(client.timeline(&req)).await?,
    };
    if json {
        return print_json(&response);
    }
    let spark = sparkline(
        &response
            .points
            .iter()
            .map(|p| p.count as u64)
            .collect::<Vec<_>>(),
    );
    println!(
        "bucket={}{} {}",
        cyan(&response.bucket),
        response
            .group_by
            .as_deref()
            .map(|g| format!(" group_by={}", muted(g)))
            .unwrap_or_default(),
        spark
    );
    for pt in &response.points {
        let group = pt
            .group
            .as_deref()
            .map(|g| format!(" [{}]", muted(g)))
            .unwrap_or_default();
        println!(
            "  {}  {:>8}{}",
            muted(&pt.bucket),
            cyan(&pt.count.to_string()),
            group
        );
    }
    Ok(())
}

pub(crate) async fn run_patterns(mode: &CliMode, args: PatternsArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.analysis().patterns(req).await?,
        CliMode::Http(client) => http_or_cancel(client.patterns(&req)).await?,
    };
    if json {
        return print_json(&response);
    }
    println!(
        "{} pattern(s) (scanned {} logs{})",
        cyan(&response.patterns.len().to_string()),
        cyan(&response.scanned.to_string()),
        if response.truncated {
            ", truncated"
        } else {
            ""
        }
    );
    for p in &response.patterns {
        println!("  {:>6}  {}", cyan(&p.count.to_string()), p.template);
    }
    Ok(())
}

pub(crate) async fn run_ingest_rate(mode: &CliMode, args: IngestRateArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.stats().ingest_rate(req).await?,
        CliMode::Http(client) => http_or_cancel(client.ingest_rate(&req)).await?,
    };
    if json {
        return print_json(&response);
    }
    let b = &response.buckets;
    println!(
        "{} ingest rate (per_sec): 1m={} 5m={} 15m={}  (counts 1m={} 5m={} 15m={}; write_blocked={})",
        if response.write_blocked {
            warn("BLOCKED")
        } else {
            muted("ok")
        },
        cyan(&format!("{:.2}", b.per_sec_1m)),
        cyan(&format!("{:.2}", b.per_sec_5m)),
        cyan(&format!("{:.2}", b.per_sec_15m)),
        cyan(&b.last_1m.to_string()),
        cyan(&b.last_5m.to_string()),
        cyan(&b.last_15m.to_string()),
        primary(&response.write_blocked.to_string()),
    );
    if let Some(hosts) = &response.by_host {
        for h in hosts {
            println!(
                "  {:<20} 1m={} 5m={} 15m={}",
                primary(&h.hostname),
                cyan(&h.last_1m.to_string()),
                cyan(&h.last_5m.to_string()),
                cyan(&h.last_15m.to_string())
            );
        }
    }
    Ok(())
}

pub(crate) async fn run_sig_list(mode: &CliMode, args: SigListArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.alerts().signatures(req).await?,
        CliMode::Http(client) => http_or_cancel(client.unaddressed_errors(&req)).await?,
    };
    if json {
        return print_json(&response);
    }
    if response.signatures.is_empty() {
        println!("No unaddressed error signatures.");
        return Ok(());
    }
    println!(
        "{} signature(s):",
        cyan(&response.signatures.len().to_string())
    );
    for sig in &response.signatures {
        let acked = if sig.acknowledged_at.is_some() {
            format!(" {}", muted("[acked]"))
        } else {
            String::new()
        };
        let hash_short = sig
            .signature_hash
            .get(..16)
            .unwrap_or(sig.signature_hash.as_str());
        println!(
            "  {:>6}x  {}  {}{}",
            cyan(&sig.total_count.to_string()),
            muted(hash_short),
            sig.template,
            acked
        );
        println!(
            "         app={} host={}",
            primary(sig.sample_app_name.as_deref().unwrap_or("-")),
            cyan(&sig.sample_hostname)
        );
    }
    Ok(())
}

pub(crate) async fn run_sig_ack(mode: &CliMode, args: SigAckArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.alerts().ack_signature(req, "cli").await?,
        CliMode::Http(client) => http_or_cancel(client.ack_error(&req)).await?,
    };
    if json {
        return print_json(&response);
    }
    println!(
        "acknowledged {} at {} by {}",
        response.signature_hash, response.acknowledged_at, response.actor
    );
    Ok(())
}

pub(crate) async fn run_sig_unack(mode: &CliMode, args: SigUnackArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.alerts().unack_signature(req, "cli").await?,
        CliMode::Http(client) => http_or_cancel(client.unack_error(&req)).await?,
    };
    if json {
        return print_json(&response);
    }
    println!(
        "unacknowledged {} at {} by {}",
        response.signature_hash, response.unacked_at, response.actor
    );
    Ok(())
}

pub(crate) async fn run_notify_recent(mode: &CliMode, args: NotifyRecentArgs) -> Result<()> {
    let json = args.json;
    let raw_limit = args.limit.unwrap_or(50);
    if !(1..=500).contains(&raw_limit) {
        anyhow::bail!("--limit must be between 1 and 500 (got {raw_limit})");
    }
    let limit = raw_limit;
    match mode {
        CliMode::Local(service) => {
            let firings = service
                .alerts()
                .notifications(cortex::app::NotificationsRecentRequest {
                    limit: Some(limit),
                    rule_id: args.rule_id,
                    hostname: None,
                    since: args.since,
                    until: None,
                })
                .await?;
            if json {
                return print_json(&firings);
            }
            if firings.is_empty() {
                println!("No recent notification firings.");
                return Ok(());
            }
            for f in &firings {
                let status = f
                    .status_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".to_string());
                println!(
                    "{} rule={} host={} status={}",
                    f.fired_at, f.rule_id, f.hostname, status
                );
            }
        }
        CliMode::Http(client) => {
            let firings =
                http_or_cancel(client.notifications_recent(limit, args.rule_id, args.since))
                    .await?;
            if json {
                return print_json(&firings);
            }
            let arr = firings.as_array().cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "unexpected response shape: expected JSON array, got {}",
                    firings
                )
            })?;
            if arr.is_empty() {
                println!("No recent notification firings.");
                return Ok(());
            }
            for f in &arr {
                let fired_at = f.get("fired_at").and_then(|v| v.as_str()).unwrap_or("-");
                let rule_id = f.get("rule_id").and_then(|v| v.as_str()).unwrap_or("-");
                let hostname = f.get("hostname").and_then(|v| v.as_str()).unwrap_or("-");
                let status = f
                    .get("status_code")
                    .and_then(|v| v.as_i64())
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".to_string());
                println!("{fired_at} rule={rule_id} host={hostname} status={status}");
            }
        }
    }
    Ok(())
}

pub(crate) async fn run_notify_test(mode: &CliMode, args: NotifyTestArgs) -> Result<()> {
    let json = args.json;
    match mode {
        CliMode::Http(client) => {
            let result = http_or_cancel(client.notifications_test(args.body)).await?;
            if json {
                return print_json(&result);
            }
            println!("{result}");
        }
        CliMode::Local(_) => {
            bail!("notify test requires --http (apprise config lives in the server process)");
        }
    }
    Ok(())
}

mod analytics;
mod gap;

pub(crate) use self::analytics::{
    run_anomalies, run_apps, run_clock_skew, run_compare, run_silent_hosts,
};
pub(crate) use self::gap::{
    run_correlate_state, run_entity_lookup, run_fleet_state, run_graph_around, run_graph_evidence,
    run_graph_explain, run_graph_rebuild, run_graph_status, run_host_state, run_topic_correlate,
};

#[cfg(test)]
#[path = "surface_tests.rs"]
mod tests;
