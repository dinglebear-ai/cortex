// Re-export anyhow::Result so compose_tests.rs can access it via `use super::*`.
#[cfg(test)]
pub(crate) use anyhow::Result;

mod docker;
mod format;
mod mutation;
mod runner;
mod types;

pub use docker::CliDockerInspect;
pub use format::{ensure_doctor_ready, mcp_projection, redact_sensitive};
pub use mutation::ComposeService;
pub use runner::ProcessRunner;
pub use types::{
    CommandOutput, CommandRunner, ComposeCommandResult, ComposeDefaults, ComposeDiagnostic,
    ComposeDryRun, ComposeInvocation, ComposeMcpDiagnostic, ComposeMcpStatus, ComposeMutation,
    ComposeOwnershipState, ComposeRuntimeState, ComposeStatus, ComposeTarget, ComposeTargetSummary,
    ContainerInfo, DiagnosticSeverity, DockerInspect, ListenerInfo, MountInfo, MutationOptions,
    PortInfo, PublicPortSummary, ResolvedComposeTarget, SystemdStatus, TargetConfidence,
    TargetSource, TimeoutCleanupStatus,
};

// Test-only re-exports of private items accessed via `use super::*` in compose_tests.rs.
#[cfg(test)]
pub(crate) use docker::{
    DockerUnavailableError, container_info_from_inspect, docker_cli_error, ss_output_has_listener,
    systemd_status_from_output,
};
#[cfg(test)]
pub(crate) use mutation::{
    DIAG_DOCKER_UNAVAILABLE, DIAG_SYSTEMD_CHECK_FAILED, DIAG_TARGET_UNRESOLVED,
    target_from_container, unresolved_code,
};

#[cfg(test)]
#[path = "compose_tests.rs"]
mod tests;
