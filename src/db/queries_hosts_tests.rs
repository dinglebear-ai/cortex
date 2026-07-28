use super::*;

fn host_entry(name: &str, first: &str, last: &str, count: i64) -> HostEntry {
    HostEntry {
        hostname: name.to_string(),
        first_seen: first.to_string(),
        last_seen: last.to_string(),
        log_count: count,
    }
}

#[test]
fn dedupe_hosts_folds_case_variants() {
    let out = dedupe_hosts(vec![
        host_entry("SHART", "2026-06-01T00:00:00Z", "2026-06-10T00:00:00Z", 10),
        host_entry("shart", "2026-06-02T00:00:00Z", "2026-06-12T00:00:00Z", 5),
    ]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hostname, "shart");
    assert_eq!(out[0].log_count, 15);
    assert_eq!(out[0].first_seen, "2026-06-01T00:00:00Z"); // earliest
    assert_eq!(out[0].last_seen, "2026-06-12T00:00:00Z"); // latest
}

#[test]
fn dedupe_hosts_folds_fqdn_into_existing_short_name() {
    let out = dedupe_hosts(vec![
        host_entry(
            "tootie",
            "2026-06-01T00:00:00Z",
            "2026-06-10T00:00:00Z",
            100,
        ),
        host_entry(
            "tootie.manatee-triceratops.ts.net",
            "2026-06-03T00:00:00Z",
            "2026-06-09T00:00:00Z",
            7,
        ),
    ]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hostname, "tootie");
    assert_eq!(out[0].log_count, 107);
}

#[test]
fn dedupe_hosts_keeps_fqdn_when_no_matching_short_name() {
    // No bare `host` row exists, so `host.docker.internal` must NOT be folded to
    // `host` — folding there would invent a merge and could mask a real machine.
    let out = dedupe_hosts(vec![host_entry(
        "host.docker.internal",
        "2026-06-01T00:00:00Z",
        "2026-06-10T00:00:00Z",
        42,
    )]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hostname, "host.docker.internal");
}

#[test]
fn dedupe_hosts_leaves_ambiguous_self_identifiers_untouched() {
    // localhost and dotless host:user forms are deferred (need source_ip).
    let out = dedupe_hosts(vec![
        host_entry(
            "localhost",
            "2026-06-01T00:00:00Z",
            "2026-06-10T00:00:00Z",
            3,
        ),
        host_entry("dookie", "2026-06-01T00:00:00Z", "2026-06-11T00:00:00Z", 9),
        host_entry(
            "dookie:jmagar",
            "2026-06-01T00:00:00Z",
            "2026-06-05T00:00:00Z",
            2,
        ),
    ]);
    let names: std::collections::HashSet<&str> = out.iter().map(|h| h.hostname.as_str()).collect();
    assert!(names.contains("localhost"));
    assert!(names.contains("dookie"));
    assert!(names.contains("dookie:jmagar")); // colon, no dot → not folded into dookie
    assert_eq!(out.len(), 3);
}

#[test]
fn dedupe_hosts_excludes_blank_hostnames() {
    let out = dedupe_hosts(vec![
        host_entry("", "2026-06-01T00:00:00Z", "2026-06-10T00:00:00Z", 3),
        host_entry("   ", "2026-06-01T00:00:00Z", "2026-06-10T00:00:00Z", 4),
        host_entry("dookie", "2026-06-01T00:00:00Z", "2026-06-11T00:00:00Z", 9),
    ]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hostname, "dookie");
}

#[test]
fn dedupe_hosts_orders_by_last_seen_desc() {
    let out = dedupe_hosts(vec![
        host_entry("alpha", "2026-06-01T00:00:00Z", "2026-06-05T00:00:00Z", 1),
        host_entry("bravo", "2026-06-01T00:00:00Z", "2026-06-20T00:00:00Z", 1),
    ]);
    assert_eq!(out[0].hostname, "bravo"); // most recent first
    assert_eq!(out[1].hostname, "alpha");
}
