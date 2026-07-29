use rmcp::model::{Prompt, PromptArgument, PromptMessage, Role};
use serde_json::Map;

#[derive(Clone, Copy)]
struct PromptSpec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    arguments: &'static [PromptArgSpec],
    body: fn(&Map<String, serde_json::Value>) -> String,
}

#[derive(Clone, Copy)]
struct PromptArgSpec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    required: bool,
}

const WINDOW_ARG: PromptArgSpec = PromptArgSpec {
    name: "window",
    title: "Window",
    description: "Investigation window, for example 'last 30 minutes' or an RFC3339 range.",
    required: false,
};

const HOST_ARG: PromptArgSpec = PromptArgSpec {
    name: "host",
    title: "Host",
    description: "Hostname or source identifier to focus on.",
    required: false,
};

const SERVICE_ARG: PromptArgSpec = PromptArgSpec {
    name: "service",
    title: "Service",
    description: "Service, application, container, or syslog app name to investigate.",
    required: false,
};

const CONTAINER_ARG: PromptArgSpec = PromptArgSpec {
    name: "container",
    title: "Container",
    description: "Docker container name or container-like app name to investigate.",
    required: false,
};

use super::prompt_text::{
    after_deploy_check_prompt, agent_change_correlation_prompt, auth_bruteforce_prompt,
    docker_container_regression_prompt, host_health_prompt, incident_triage_prompt,
    network_dns_failure_prompt, noise_reduction_prompt, security_auth_review_prompt,
    service_outage_prompt, storage_pressure_prompt, syslog_forwarding_gap_prompt,
};

const PROMPTS: &[PromptSpec] = &[
    PromptSpec {
        name: "infra.incident-triage",
        title: "Incident Triage",
        description: "Build a timeline, scope, and next actions for a suspected infrastructure incident.",
        arguments: &[WINDOW_ARG, HOST_ARG, SERVICE_ARG],
        body: incident_triage_prompt,
    },
    PromptSpec {
        name: "infra.host-health",
        title: "Host Health Check",
        description: "Investigate whether one host is silent, noisy, clock-skewed, or producing errors.",
        arguments: &[
            PromptArgSpec {
                required: true,
                ..HOST_ARG
            },
            WINDOW_ARG,
        ],
        body: host_health_prompt,
    },
    PromptSpec {
        name: "infra.service-outage",
        title: "Service Outage",
        description: "Debug a service, app, or container outage from logs and correlated host events.",
        arguments: &[
            PromptArgSpec {
                required: true,
                ..SERVICE_ARG
            },
            HOST_ARG,
            WINDOW_ARG,
        ],
        body: service_outage_prompt,
    },
    PromptSpec {
        name: "infra.security-auth-review",
        title: "Security Auth Review",
        description: "Review authentication failures, bans, suspicious IPs, and related infrastructure context.",
        arguments: &[
            WINDOW_ARG,
            PromptArgSpec {
                name: "actor",
                title: "Actor",
                description: "Optional username, email, token subject, or client IP to focus on.",
                required: false,
            },
            HOST_ARG,
        ],
        body: security_auth_review_prompt,
    },
    PromptSpec {
        name: "infra.noise-reduction",
        title: "Noise Reduction",
        description: "Find repeated log patterns and propose safe alerting or suppression changes.",
        arguments: &[WINDOW_ARG, HOST_ARG, SERVICE_ARG],
        body: noise_reduction_prompt,
    },
    PromptSpec {
        name: "infra.agent-change-correlation",
        title: "Agent Change Correlation",
        description: "Correlate AI agent activity with infrastructure errors and regressions.",
        arguments: &[
            PromptArgSpec {
                name: "project",
                title: "Project",
                description: "Exact AI project path to inspect, if known.",
                required: false,
            },
            PromptArgSpec {
                name: "session_id",
                title: "Session ID",
                description: "AI transcript session id to focus on, if known.",
                required: false,
            },
            WINDOW_ARG,
            HOST_ARG,
            SERVICE_ARG,
        ],
        body: agent_change_correlation_prompt,
    },
    PromptSpec {
        name: "infra.docker-container-regression",
        title: "Docker Container Regression",
        description: "Investigate a container restart, healthcheck, image, or Compose regression.",
        arguments: &[CONTAINER_ARG, HOST_ARG, SERVICE_ARG, WINDOW_ARG],
        body: docker_container_regression_prompt,
    },
    PromptSpec {
        name: "infra.network-dns-failure",
        title: "Network DNS Failure",
        description: "Debug DNS, proxy, firewall, and network reachability failures from logs.",
        arguments: &[HOST_ARG, SERVICE_ARG, WINDOW_ARG],
        body: network_dns_failure_prompt,
    },
    PromptSpec {
        name: "infra.storage-pressure",
        title: "Storage Pressure",
        description: "Investigate disk pressure, database growth, cleanup, and write-block risk.",
        arguments: &[HOST_ARG, SERVICE_ARG, WINDOW_ARG],
        body: storage_pressure_prompt,
    },
    PromptSpec {
        name: "infra.auth-bruteforce",
        title: "Auth Bruteforce",
        description: "Investigate repeated authentication failures, bans, and suspicious sources.",
        arguments: &[
            WINDOW_ARG,
            PromptArgSpec {
                name: "actor",
                title: "Actor",
                description: "Optional username, email, token subject, or client IP to focus on.",
                required: false,
            },
            HOST_ARG,
            SERVICE_ARG,
        ],
        body: auth_bruteforce_prompt,
    },
    PromptSpec {
        name: "infra.syslog-forwarding-gap",
        title: "Syslog Forwarding Gap",
        description: "Investigate missing, stale, spoofed, or delayed syslog forwarding.",
        arguments: &[HOST_ARG, WINDOW_ARG],
        body: syslog_forwarding_gap_prompt,
    },
    PromptSpec {
        name: "infra.after-deploy-check",
        title: "After Deploy Check",
        description: "Verify service health and regressions after a deployment or config change.",
        arguments: &[SERVICE_ARG, HOST_ARG, WINDOW_ARG],
        body: after_deploy_check_prompt,
    },
];

pub(in crate::mcp) fn prompt_definitions() -> Vec<Prompt> {
    PROMPTS
        .iter()
        .map(|spec| {
            Prompt::new(
                spec.name,
                Some(spec.description),
                Some(
                    spec.arguments
                        .iter()
                        .map(|arg| {
                            PromptArgument::new(arg.name)
                                .with_title(arg.title)
                                .with_description(arg.description)
                                .with_required(arg.required)
                        })
                        .collect(),
                ),
            )
            .with_title(spec.title)
        })
        .collect()
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;

pub(in crate::mcp) fn get_prompt(
    name: &str,
    arguments: Option<&Map<String, serde_json::Value>>,
) -> Option<(String, Vec<PromptMessage>)> {
    let spec = PROMPTS.iter().find(|spec| spec.name == name)?;
    let empty = Map::new();
    let args = arguments.unwrap_or(&empty);
    let text = (spec.body)(args);
    Some((
        spec.description.to_string(),
        vec![PromptMessage::new_text(Role::User, text)],
    ))
}
