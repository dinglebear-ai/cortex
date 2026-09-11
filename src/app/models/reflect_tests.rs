use super::*;

fn incident(
    kind: ReflectKind,
    id: &str,
    score: f64,
    label: &str,
    last_seen: &str,
) -> ReflectIncident {
    ReflectIncident {
        kind,
        incident_id: id.to_string(),
        target: format!("target-{id}"),
        target_key: format!("key-{id}"),
        target_detail: None,
        tool: "claude".to_string(),
        project: "/p".to_string(),
        session_id: "s".to_string(),
        last_seen: last_seen.to_string(),
        priority_score: score,
        priority_label: label.to_string(),
        signals_present: vec![],
    }
}

#[test]
fn kind_parse_round_trips_and_rejects_others() {
    for kind in ReflectKind::ALL {
        assert_eq!(ReflectKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(ReflectKind::parse("abuse"), None);
    assert_eq!(ReflectKind::parse(""), None);
}

#[test]
fn hook_assess_subcommand_is_plural() {
    assert_eq!(ReflectKind::Skill.assess_subcommand(), "skill");
    assert_eq!(ReflectKind::Mcp.assess_subcommand(), "mcp");
    assert_eq!(ReflectKind::Hook.assess_subcommand(), "hooks");
}

#[test]
fn enums_serialize_as_documented() {
    assert_eq!(serde_json::to_string(&ReflectKind::Mcp).unwrap(), "\"mcp\"");
    assert_eq!(
        serde_json::to_string(&ReflectMode::ReportAndLlm).unwrap(),
        "\"report_and_llm\""
    );
}

#[test]
fn rank_orders_by_score_then_recency_then_id() {
    let ranked = rank_reflect_incidents(vec![
        incident(ReflectKind::Hook, "c", 10.0, "low", "2026-09-10T00:00:00Z"),
        incident(
            ReflectKind::Skill,
            "b",
            40.0,
            "high",
            "2026-09-09T00:00:00Z",
        ),
        incident(ReflectKind::Mcp, "a", 40.0, "high", "2026-09-09T00:00:00Z"),
        incident(ReflectKind::Mcp, "d", 40.0, "high", "2026-09-10T00:00:00Z"),
    ]);
    let ids: Vec<&str> = ranked.iter().map(|i| i.incident_id.as_str()).collect();
    assert_eq!(ids, ["d", "a", "b", "c"]);
}

#[test]
fn summary_counts_listed_totals_and_truncation_per_kind() {
    let incidents = vec![
        incident(ReflectKind::Skill, "a", 70.0, "critical", "t"),
        incident(ReflectKind::Skill, "b", 20.0, "medium", "t"),
        incident(ReflectKind::Hook, "c", 5.0, "low", "t"),
    ];
    let listings = [
        ReflectKindListing {
            kind: ReflectKind::Skill,
            total: 250,
            truncated: true,
        },
        ReflectKindListing {
            kind: ReflectKind::Mcp,
            total: 0,
            truncated: false,
        },
        ReflectKindListing {
            kind: ReflectKind::Hook,
            total: 1,
            truncated: false,
        },
    ];
    let summary = summarize_reflect_incidents(&listings, &incidents);
    assert_eq!(summary.len(), 3);
    let skill = &summary[0];
    assert_eq!(
        (skill.kind, skill.listed, skill.total, skill.truncated),
        (ReflectKind::Skill, 2, 250, true)
    );
    assert_eq!((skill.critical, skill.medium), (1, 1));
    assert_eq!((summary[1].listed, summary[1].total), (0, 0));
    assert_eq!(
        (summary[2].listed, summary[2].total, summary[2].low),
        (1, 1, 1)
    );
}

#[test]
fn summary_total_is_never_below_listed() {
    let incidents = vec![incident(ReflectKind::Mcp, "a", 1.0, "low", "t")];
    let listings = [ReflectKindListing {
        kind: ReflectKind::Mcp,
        total: 0,
        truncated: false,
    }];
    assert_eq!(
        summarize_reflect_incidents(&listings, &incidents)[0].total,
        1
    );
}

#[test]
fn assess_command_includes_the_window() {
    let hook = incident(ReflectKind::Hook, "abc", 1.0, "low", "t");
    assert_eq!(
        hook.assess_command(None, None),
        "cortex assess hooks --incident-id abc"
    );
    assert_eq!(
        hook.assess_command(Some("S"), Some("U")),
        "cortex assess hooks --incident-id abc --since S --until U"
    );
}
