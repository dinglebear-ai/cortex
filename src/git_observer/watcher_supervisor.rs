//! Overflow and periodic Git reconciliation supervisor.

use super::GitWatchAction;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitWatchSupervisorOptions {
    pub overflow_min_interval: Duration,
    pub periodic_interval: Duration,
}

impl Default for GitWatchSupervisorOptions {
    fn default() -> Self {
        Self {
            overflow_min_interval: Duration::from_secs(60),
            periodic_interval: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitFullReconcileReason {
    Overflow,
    Periodic,
    OverflowAndPeriodic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitWatchScheduledAction {
    ReconcileRepository { repository_key: String },
    DiscoverRepositories { project_root: PathBuf },
    FullReconcile { reason: GitFullReconcileReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitWatchSupervisorErrorKind {
    InvalidOverflowMinInterval,
    InvalidPeriodicInterval,
    IntervalOverflow,
    NoReconcileInFlight,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWatchSupervisorError {
    pub kind: GitWatchSupervisorErrorKind,
}

impl fmt::Display for GitWatchSupervisorErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOverflowMinInterval => {
                formatter.write_str("overflow_min_interval must be positive")
            }
            Self::InvalidPeriodicInterval => {
                formatter.write_str("periodic_interval must be positive")
            }
            Self::IntervalOverflow => formatter.write_str("scheduler interval overflowed Instant"),
            Self::NoReconcileInFlight => formatter.write_str("no full reconcile is in flight"),
        }
    }
}

impl fmt::Display for GitWatchSupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Git watch supervisor: {}", self.kind)
    }
}

impl std::error::Error for GitWatchSupervisorError {}

fn schedule_after(now: Instant, interval: Duration) -> Result<Instant, GitWatchSupervisorError> {
    now.checked_add(interval).ok_or(GitWatchSupervisorError {
        kind: GitWatchSupervisorErrorKind::IntervalOverflow,
    })
}

#[derive(Debug, Clone)]
pub struct GitWatchSupervisorHandle {
    overflow_pending: Arc<AtomicBool>,
}

impl GitWatchSupervisorHandle {
    pub fn signal_overflow(&self) {
        self.overflow_pending.store(true, Ordering::Release);
    }

    pub fn overflow_pending(&self) -> bool {
        self.overflow_pending.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub struct GitWatchSupervisor {
    overflow_pending: Arc<AtomicBool>,
    options: GitWatchSupervisorOptions,
    next_overflow_at: Instant,
    next_periodic_at: Instant,
    full_reconcile_in_flight: bool,
}

pub fn git_watch_supervisor(
    start: Instant,
    options: GitWatchSupervisorOptions,
) -> Result<(GitWatchSupervisorHandle, GitWatchSupervisor), GitWatchSupervisorError> {
    if options.overflow_min_interval.is_zero() {
        return Err(GitWatchSupervisorError {
            kind: GitWatchSupervisorErrorKind::InvalidOverflowMinInterval,
        });
    }
    if options.periodic_interval.is_zero() {
        return Err(GitWatchSupervisorError {
            kind: GitWatchSupervisorErrorKind::InvalidPeriodicInterval,
        });
    }
    let next_periodic_at = schedule_after(start, options.periodic_interval)?;
    schedule_after(start, options.overflow_min_interval)?;
    let overflow_pending = Arc::new(AtomicBool::new(false));
    Ok((
        GitWatchSupervisorHandle {
            overflow_pending: Arc::clone(&overflow_pending),
        },
        GitWatchSupervisor {
            overflow_pending,
            options,
            next_overflow_at: start,
            next_periodic_at,
            full_reconcile_in_flight: false,
        },
    ))
}

impl GitWatchSupervisor {
    fn direct_action(action: GitWatchAction) -> Option<GitWatchScheduledAction> {
        match action {
            GitWatchAction::ReconcileRepository { repository_key } => {
                Some(GitWatchScheduledAction::ReconcileRepository { repository_key })
            }
            GitWatchAction::DiscoverRepositories { project_root } => {
                Some(GitWatchScheduledAction::DiscoverRepositories { project_root })
            }
            GitWatchAction::FullReconcile => None,
        }
    }

    fn schedule_full_reconcile(
        &mut self,
        now: Instant,
        overflow_pending: bool,
        periodic_due: bool,
    ) -> Result<GitFullReconcileReason, GitWatchSupervisorError> {
        let reason = match (overflow_pending, periodic_due) {
            (true, true) => GitFullReconcileReason::OverflowAndPeriodic,
            (true, false) => GitFullReconcileReason::Overflow,
            (false, true) => GitFullReconcileReason::Periodic,
            (false, false) => unreachable!("caller requires one full reconcile reason"),
        };
        self.overflow_pending.store(false, Ordering::Release);
        self.next_overflow_at = schedule_after(now, self.options.overflow_min_interval)?;
        self.next_periodic_at = schedule_after(now, self.options.periodic_interval)?;
        self.full_reconcile_in_flight = true;
        Ok(reason)
    }

    pub fn poll<I>(&mut self, now: Instant, queue_actions: I) -> Vec<GitWatchScheduledAction>
    where
        I: IntoIterator<Item = GitWatchAction>,
    {
        let mut actions = Vec::new();
        for action in queue_actions {
            if action == GitWatchAction::FullReconcile {
                self.overflow_pending.store(true, Ordering::Release);
            } else if let Some(action) = Self::direct_action(action) {
                actions.push(action);
            }
        }
        if self.full_reconcile_in_flight {
            return actions;
        }

        let overflow_pending = self.overflow_pending.load(Ordering::Acquire);
        let overflow_due = overflow_pending && now >= self.next_overflow_at;
        let periodic_due = now >= self.next_periodic_at;
        if !periodic_due && !overflow_due {
            return actions;
        }
        let overflow_satisfied = overflow_pending && (overflow_due || periodic_due);
        let reason = self
            .schedule_full_reconcile(now, overflow_satisfied, periodic_due)
            .expect("validated scheduler intervals must remain representable");
        actions.push(GitWatchScheduledAction::FullReconcile { reason });
        actions
    }

    pub fn complete_full_reconcile(
        &mut self,
        succeeded: bool,
        completed_at: Instant,
    ) -> Result<(), GitWatchSupervisorError> {
        if !self.full_reconcile_in_flight {
            return Err(GitWatchSupervisorError {
                kind: GitWatchSupervisorErrorKind::NoReconcileInFlight,
            });
        }
        self.full_reconcile_in_flight = false;
        if !succeeded {
            self.overflow_pending.store(true, Ordering::Release);
            self.next_overflow_at =
                schedule_after(completed_at, self.options.overflow_min_interval)?;
        }
        Ok(())
    }

    pub fn full_reconcile_in_flight(&self) -> bool {
        self.full_reconcile_in_flight
    }
}

#[cfg(test)]
#[path = "watcher_supervisor_tests.rs"]
mod tests;
