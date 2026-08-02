//! Pure Agent Observatory lifecycle reduction.

use crate::config::AgentObservatoryConfig;
use crate::db::agent_observatory::RunStatus;
use chrono::{DateTime, Utc};

/// Time windows controlling activity, staleness, and abandonment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleWindows {
    /// Activity at or inside this age remains active.
    pub active_window_secs: u64,
    /// Activity older than the active window and at or inside this age is idle.
    pub stale_after_secs: u64,
    /// Activity at or beyond this age may be abandoned when the process is not live.
    pub abandoned_after_secs: u64,
}

impl From<&AgentObservatoryConfig> for LifecycleWindows {
    fn from(config: &AgentObservatoryConfig) -> Self {
        Self {
            active_window_secs: config.active_window_secs,
            stale_after_secs: config.stale_after_secs,
            abandoned_after_secs: config.abandoned_after_secs,
        }
    }
}

/// Current open-wait category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    /// The provider is waiting for user permission or confirmation.
    Permission,
    /// The provider is waiting for an external tool or operation.
    Tool,
}

/// Evidence that the run currently has an unresolved wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenWaitEvidence {
    /// Wait classification.
    pub kind: WaitKind,
    /// Time the open wait was observed.
    pub observed_at: DateTime<Utc>,
}

/// Durable reason accompanying the reduced lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleReason {
    ExplicitSuccess,
    ExplicitFailure,
    PermissionWait,
    ToolWait,
    RecentActivity,
    IdleTimeout,
    StaleTimeout,
    AbandonedTimeout,
    NoActivityYet,
}

impl LifecycleReason {
    /// Stable database/API representation from the Agent Observatory contract.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitSuccess => "explicit_success",
            Self::ExplicitFailure => "explicit_failure",
            Self::PermissionWait => "permission_wait",
            Self::ToolWait => "tool_wait",
            Self::RecentActivity => "recent_activity",
            Self::IdleTimeout => "idle_timeout",
            Self::StaleTimeout => "stale_timeout",
            Self::AbandonedTimeout => "abandoned_timeout",
            Self::NoActivityYet => "no_activity_yet",
        }
    }
}

/// Evidence consumed by the pure lifecycle reducer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleEvidence {
    pub started_at: DateTime<Utc>,
    pub explicit_failure_at: Option<DateTime<Utc>>,
    pub explicit_success_at: Option<DateTime<Utc>>,
    pub open_wait: Option<OpenWaitEvidence>,
    pub latest_activity_at: Option<DateTime<Utc>>,
    pub process_live: Option<bool>,
}

impl Default for LifecycleEvidence {
    fn default() -> Self {
        Self {
            started_at: DateTime::from_timestamp(0, 0).expect("Unix epoch is valid"),
            explicit_failure_at: None,
            explicit_success_at: None,
            open_wait: None,
            latest_activity_at: None,
            process_live: None,
        }
    }
}

/// Materialized lifecycle state stored on an Agent Observatory run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleState {
    pub status: RunStatus,
    pub reason: LifecycleReason,
    pub observed_at: DateTime<Utc>,
}

/// Lifecycle reducer output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleDecision {
    pub state: LifecycleState,
    pub changed: bool,
}

fn timeout_state(status: RunStatus, reason: LifecycleReason, now: DateTime<Utc>) -> LifecycleState {
    LifecycleState {
        status,
        reason,
        observed_at: now,
    }
}

fn candidate_state(
    now: DateTime<Utc>,
    evidence: &LifecycleEvidence,
    windows: LifecycleWindows,
) -> LifecycleState {
    if let Some(observed_at) = evidence.explicit_failure_at {
        return LifecycleState {
            status: RunStatus::Failed,
            reason: LifecycleReason::ExplicitFailure,
            observed_at,
        };
    }
    if let Some(observed_at) = evidence.explicit_success_at {
        return LifecycleState {
            status: RunStatus::Completed,
            reason: LifecycleReason::ExplicitSuccess,
            observed_at,
        };
    }
    if let Some(wait) = evidence.open_wait {
        let reason = match wait.kind {
            WaitKind::Permission => LifecycleReason::PermissionWait,
            WaitKind::Tool => LifecycleReason::ToolWait,
        };
        return LifecycleState {
            status: RunStatus::Waiting,
            reason,
            observed_at: wait.observed_at,
        };
    }

    let Some(latest_activity_at) = evidence.latest_activity_at else {
        return LifecycleState {
            status: RunStatus::Starting,
            reason: LifecycleReason::NoActivityYet,
            observed_at: evidence.started_at,
        };
    };

    let age_secs = now
        .signed_duration_since(latest_activity_at)
        .num_seconds()
        .max(0) as u64;

    if age_secs <= windows.active_window_secs {
        return LifecycleState {
            status: RunStatus::Active,
            reason: LifecycleReason::RecentActivity,
            observed_at: latest_activity_at,
        };
    }
    if age_secs <= windows.stale_after_secs {
        return timeout_state(RunStatus::Idle, LifecycleReason::IdleTimeout, now);
    }
    if age_secs < windows.abandoned_after_secs {
        return timeout_state(RunStatus::Stale, LifecycleReason::StaleTimeout, now);
    }
    if evidence.process_live == Some(false) {
        timeout_state(RunStatus::Abandoned, LifecycleReason::AbandonedTimeout, now)
    } else {
        timeout_state(RunStatus::Stale, LifecycleReason::StaleTimeout, now)
    }
}

/// Reduce lifecycle evidence into a deterministic materialized state.
///
/// When the status and reason are unchanged, the prior observation timestamp is
/// retained and `changed` is false so polling does not create needless writes.
pub fn reduce_lifecycle(
    now: DateTime<Utc>,
    evidence: &LifecycleEvidence,
    windows: LifecycleWindows,
    previous: Option<&LifecycleState>,
) -> LifecycleDecision {
    let candidate = candidate_state(now, evidence, windows);
    if let Some(previous) = previous
        && previous.status == candidate.status
        && previous.reason == candidate.reason
    {
        return LifecycleDecision {
            state: previous.clone(),
            changed: false,
        };
    }
    LifecycleDecision {
        state: candidate,
        changed: true,
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
