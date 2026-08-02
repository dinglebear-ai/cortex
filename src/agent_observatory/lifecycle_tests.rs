use super::{
    LifecycleDecision, LifecycleEvidence, LifecycleReason, LifecycleState, LifecycleWindows,
    OpenWaitEvidence, WaitKind, reduce_lifecycle,
};
use crate::config::AgentObservatoryConfig;
use crate::db::agent_observatory::RunStatus;
use chrono::{DateTime, TimeDelta, Utc};

fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn now() -> DateTime<Utc> {
    ts("2026-08-02T16:00:00Z")
}

fn windows() -> LifecycleWindows {
    LifecycleWindows {
        active_window_secs: 15,
        stale_after_secs: 300,
        abandoned_after_secs: 86_400,
    }
}

fn evidence_with_activity(age_secs: i64) -> LifecycleEvidence {
    LifecycleEvidence {
        started_at: now() - TimeDelta::hours(1),
        latest_activity_at: Some(now() - TimeDelta::seconds(age_secs)),
        ..LifecycleEvidence::default()
    }
}

fn assert_decision(decision: &LifecycleDecision, status: RunStatus, reason: LifecycleReason) {
    assert_eq!(decision.state.status, status);
    assert_eq!(decision.state.reason, reason);
    assert!(decision.changed);
}

#[test]
fn explicit_failure_has_highest_precedence() {
    let failure_at = now() - TimeDelta::seconds(5);
    let evidence = LifecycleEvidence {
        started_at: now() - TimeDelta::hours(1),
        explicit_failure_at: Some(failure_at),
        explicit_success_at: Some(now() - TimeDelta::seconds(1)),
        open_wait: Some(OpenWaitEvidence {
            kind: WaitKind::Permission,
            observed_at: now(),
        }),
        latest_activity_at: Some(now()),
        process_live: Some(true),
    };

    let decision = reduce_lifecycle(now(), &evidence, windows(), None);
    assert_decision(
        &decision,
        RunStatus::Failed,
        LifecycleReason::ExplicitFailure,
    );
    assert_eq!(decision.state.observed_at, failure_at);
}

#[test]
fn explicit_success_precedes_waiting_and_activity() {
    let success_at = now() - TimeDelta::seconds(4);
    let evidence = LifecycleEvidence {
        started_at: now() - TimeDelta::hours(1),
        explicit_success_at: Some(success_at),
        open_wait: Some(OpenWaitEvidence {
            kind: WaitKind::Tool,
            observed_at: now(),
        }),
        latest_activity_at: Some(now()),
        ..LifecycleEvidence::default()
    };

    let decision = reduce_lifecycle(now(), &evidence, windows(), None);
    assert_decision(
        &decision,
        RunStatus::Completed,
        LifecycleReason::ExplicitSuccess,
    );
    assert_eq!(decision.state.observed_at, success_at);
}

#[test]
fn open_wait_distinguishes_permission_and_tool_reasons() {
    for (kind, reason) in [
        (WaitKind::Permission, LifecycleReason::PermissionWait),
        (WaitKind::Tool, LifecycleReason::ToolWait),
    ] {
        let wait_at = now() - TimeDelta::seconds(3);
        let evidence = LifecycleEvidence {
            started_at: now() - TimeDelta::minutes(1),
            open_wait: Some(OpenWaitEvidence {
                kind,
                observed_at: wait_at,
            }),
            latest_activity_at: Some(now()),
            ..LifecycleEvidence::default()
        };
        let decision = reduce_lifecycle(now(), &evidence, windows(), None);
        assert_decision(&decision, RunStatus::Waiting, reason);
        assert_eq!(decision.state.observed_at, wait_at);
    }
}

#[test]
fn missing_substantive_activity_is_starting_not_terminal() {
    let started_at = now() - TimeDelta::seconds(10);
    let evidence = LifecycleEvidence {
        started_at,
        process_live: None,
        ..LifecycleEvidence::default()
    };
    let decision = reduce_lifecycle(now(), &evidence, windows(), None);
    assert_decision(
        &decision,
        RunStatus::Starting,
        LifecycleReason::NoActivityYet,
    );
    assert_eq!(decision.state.observed_at, started_at);
}

#[test]
fn active_and_idle_threshold_seconds_are_inclusive() {
    let at_active = reduce_lifecycle(now(), &evidence_with_activity(15), windows(), None);
    assert_decision(
        &at_active,
        RunStatus::Active,
        LifecycleReason::RecentActivity,
    );

    let after_active = reduce_lifecycle(now(), &evidence_with_activity(16), windows(), None);
    assert_decision(&after_active, RunStatus::Idle, LifecycleReason::IdleTimeout);

    let at_stale = reduce_lifecycle(now(), &evidence_with_activity(300), windows(), None);
    assert_decision(&at_stale, RunStatus::Idle, LifecycleReason::IdleTimeout);

    let after_stale = reduce_lifecycle(now(), &evidence_with_activity(301), windows(), None);
    assert_decision(
        &after_stale,
        RunStatus::Stale,
        LifecycleReason::StaleTimeout,
    );
}

#[test]
fn abandonment_requires_threshold_and_explicit_not_live_evidence() {
    let before = LifecycleEvidence {
        process_live: Some(false),
        ..evidence_with_activity(86_399)
    };
    assert_decision(
        &reduce_lifecycle(now(), &before, windows(), None),
        RunStatus::Stale,
        LifecycleReason::StaleTimeout,
    );

    let abandoned = LifecycleEvidence {
        process_live: Some(false),
        ..evidence_with_activity(86_400)
    };
    assert_decision(
        &reduce_lifecycle(now(), &abandoned, windows(), None),
        RunStatus::Abandoned,
        LifecycleReason::AbandonedTimeout,
    );
}

#[test]
fn live_or_unobserved_process_evidence_never_forces_abandoned() {
    for process_live in [Some(true), None] {
        let evidence = LifecycleEvidence {
            process_live,
            ..evidence_with_activity(100_000)
        };
        assert_decision(
            &reduce_lifecycle(now(), &evidence, windows(), None),
            RunStatus::Stale,
            LifecycleReason::StaleTimeout,
        );
    }
}

#[test]
fn unchanged_materialized_state_preserves_observed_at_and_skips_write() {
    let previous_at = now() - TimeDelta::minutes(3);
    let previous = LifecycleState {
        status: RunStatus::Idle,
        reason: LifecycleReason::IdleTimeout,
        observed_at: previous_at,
    };
    let evidence = evidence_with_activity(120);

    let decision = reduce_lifecycle(now(), &evidence, windows(), Some(&previous));
    assert_eq!(decision.state, previous);
    assert!(!decision.changed);
}

#[test]
fn status_transition_uses_current_time_for_timeout_observation() {
    let previous = LifecycleState {
        status: RunStatus::Active,
        reason: LifecycleReason::RecentActivity,
        observed_at: now() - TimeDelta::minutes(5),
    };
    let evidence = evidence_with_activity(120);

    let decision = reduce_lifecycle(now(), &evidence, windows(), Some(&previous));
    assert_decision(&decision, RunStatus::Idle, LifecycleReason::IdleTimeout);
    assert_eq!(decision.state.observed_at, now());
}

#[test]
fn future_activity_is_clamped_to_zero_age() {
    let evidence = LifecycleEvidence {
        started_at: now(),
        latest_activity_at: Some(now() + TimeDelta::seconds(5)),
        ..LifecycleEvidence::default()
    };
    assert_decision(
        &reduce_lifecycle(now(), &evidence, windows(), None),
        RunStatus::Active,
        LifecycleReason::RecentActivity,
    );
}

#[test]
fn lifecycle_reason_codes_match_the_frozen_contract() {
    let cases = [
        (LifecycleReason::ExplicitSuccess, "explicit_success"),
        (LifecycleReason::ExplicitFailure, "explicit_failure"),
        (LifecycleReason::PermissionWait, "permission_wait"),
        (LifecycleReason::ToolWait, "tool_wait"),
        (LifecycleReason::RecentActivity, "recent_activity"),
        (LifecycleReason::IdleTimeout, "idle_timeout"),
        (LifecycleReason::StaleTimeout, "stale_timeout"),
        (LifecycleReason::AbandonedTimeout, "abandoned_timeout"),
        (LifecycleReason::NoActivityYet, "no_activity_yet"),
    ];
    for (reason, expected) in cases {
        assert_eq!(reason.as_str(), expected);
    }
}

#[test]
fn default_config_windows_match_reducer_contract() {
    let config = AgentObservatoryConfig::default();
    assert_eq!(LifecycleWindows::from(&config), windows());
}
