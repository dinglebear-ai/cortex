//! Canonical CLI flag metadata, shared by the parser, completion, and help.

/// One CLI flag for an action. `value_kind` drives dynamic completion.
#[derive(Debug, Clone, Copy)]
pub struct FlagSpec {
    /// Canonical long flag, including leading dashes, e.g. "--host".
    pub flag: &'static str,
    /// Optional short alias, e.g. "-n". Empty string = none.
    pub short: &'static str,
    /// One-line help.
    pub help: &'static str,
    /// Completion source for the flag's value.
    pub value_kind: ValueKind,
}

/// What completes after a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// No value (boolean flag).
    None,
    /// Free text (no candidates).
    Text,
    /// Live hostnames from the DB.
    Host,
    /// Live app names from the DB.
    App,
    /// Live source identifiers from the DB.
    Source,
    /// Fixed enum candidates.
    Enum(&'static [&'static str]),
    /// A time value (offers relative hints).
    Time,
}

/// Zero-flag defaults advertised for an action when the user omits them.
///
/// Kept deliberately small: the only knobs worth defaulting at the CLI layer
/// are the result `limit` and the start-of-window `since`. Request handling
/// still resolves defaults in the shared service layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Defaults {
    /// Default `--limit` when the user passes none (`None` = server default).
    pub limit: Option<u32>,
    /// Default `--since` window when unset, expressed as a relative literal the
    /// time parser understands (e.g. `"1h"`). `None` = unbounded.
    pub since: Option<&'static str>,
}

impl Defaults {
    /// Const constructor for the empty (no-defaults) case, usable inside the
    /// `const ACTION_SPECS` table where `Default::default()` cannot be called.
    pub const fn new() -> Self {
        Self {
            limit: None,
            since: None,
        }
    }
}

pub(super) const SEVERITIES: &[&str] = &[
    "emerg", "alert", "crit", "err", "warning", "notice", "info", "debug",
];

/// Flags shared by the log-query actions (search/filter/tail/errors/...).
pub(super) const COMMON_LOG_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        flag: "--host",
        short: "",
        help: "Filter by hostname",
        value_kind: ValueKind::Host,
    },
    FlagSpec {
        flag: "--app",
        short: "",
        help: "Filter by app/program name",
        value_kind: ValueKind::App,
    },
    FlagSpec {
        flag: "--source",
        short: "",
        help: "Filter by source id (IP:port or docker://...)",
        value_kind: ValueKind::Source,
    },
    FlagSpec {
        flag: "--severity",
        short: "-s",
        help: "Minimum severity",
        value_kind: ValueKind::Enum(SEVERITIES),
    },
    FlagSpec {
        flag: "--since",
        short: "",
        help: "Start of window (1h, 2d, yesterday, RFC3339)",
        value_kind: ValueKind::Time,
    },
    FlagSpec {
        flag: "--until",
        short: "",
        help: "End of window",
        value_kind: ValueKind::Time,
    },
    FlagSpec {
        flag: "--limit",
        short: "-n",
        help: "Max results",
        value_kind: ValueKind::Text,
    },
    FlagSpec {
        flag: "--json",
        short: "",
        help: "JSON output",
        value_kind: ValueKind::None,
    },
];

pub(super) const HOST_STATE_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        flag: "--host",
        short: "",
        help: "Select a hostname; defaults to the freshest host",
        value_kind: ValueKind::Host,
    },
    FlagSpec {
        flag: "--host-id",
        short: "",
        help: "Select a stable heartbeat host id",
        value_kind: ValueKind::Text,
    },
    FlagSpec {
        flag: "--since",
        short: "",
        help: "Start of window",
        value_kind: ValueKind::Time,
    },
    FlagSpec {
        flag: "--limit",
        short: "-n",
        help: "Maximum heartbeat samples",
        value_kind: ValueKind::Text,
    },
    FlagSpec {
        flag: "--json",
        short: "",
        help: "JSON output",
        value_kind: ValueKind::None,
    },
];

/// Flags for `topic_correlate`: graph-anchored universal correlation.
pub(super) const TOPIC_CORRELATE_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        flag: "--since",
        short: "",
        help: "Start of window (1h, 2d, yesterday, RFC3339)",
        value_kind: ValueKind::Time,
    },
    FlagSpec {
        flag: "--until",
        short: "",
        help: "End of window",
        value_kind: ValueKind::Time,
    },
    FlagSpec {
        flag: "--depth",
        short: "",
        help: "Graph traversal depth (default 2, max 6)",
        value_kind: ValueKind::Text,
    },
    FlagSpec {
        flag: "--source-kinds",
        short: "",
        help: "Comma-separated source kinds (e.g. docker-stream,agent-command)",
        value_kind: ValueKind::Text,
    },
    FlagSpec {
        flag: "--limit",
        short: "-n",
        help: "Max timeline rows",
        value_kind: ValueKind::Text,
    },
    FlagSpec {
        flag: "--json",
        short: "",
        help: "JSON output",
        value_kind: ValueKind::None,
    },
];

const AI_SKILL_FLAG: FlagSpec = FlagSpec {
    flag: "--skill",
    short: "",
    help: "Filter by skill name",
    value_kind: ValueKind::Text,
};
const AI_PLUGIN_FLAG: FlagSpec = FlagSpec {
    flag: "--plugin",
    short: "",
    help: "Filter by plugin name",
    value_kind: ValueKind::Text,
};
const AI_TOOL_FLAG: FlagSpec = FlagSpec {
    flag: "--tool",
    short: "",
    help: "Filter by AI tool",
    value_kind: ValueKind::Text,
};
const AI_TOOL_NAME_FLAG: FlagSpec = FlagSpec {
    flag: "--tool-name",
    short: "",
    help: "Filter by raw tool name",
    value_kind: ValueKind::Text,
};
const AI_MCP_SERVER_FLAG: FlagSpec = FlagSpec {
    flag: "--mcp-server",
    short: "",
    help: "Filter by MCP server",
    value_kind: ValueKind::Text,
};
const AI_MCP_TOOL_FLAG: FlagSpec = FlagSpec {
    flag: "--mcp-tool",
    short: "",
    help: "Filter by MCP tool",
    value_kind: ValueKind::Text,
};
const AI_HOOK_EVENT_FLAG: FlagSpec = FlagSpec {
    flag: "--hook-event",
    short: "",
    help: "Filter by hook event",
    value_kind: ValueKind::Text,
};
const AI_HOOK_FLAG: FlagSpec = FlagSpec {
    flag: "--hook",
    short: "",
    help: "Filter by hook name",
    value_kind: ValueKind::Text,
};
const AI_HOOK_SOURCE_FLAG: FlagSpec = FlagSpec {
    flag: "--hook-source",
    short: "",
    help: "Filter by hook source",
    value_kind: ValueKind::Text,
};
const AI_STATUS_FLAG: FlagSpec = FlagSpec {
    flag: "--status",
    short: "",
    help: "Filter by event status",
    value_kind: ValueKind::Text,
};
const AI_EVIDENCE_KIND_FLAG: FlagSpec = FlagSpec {
    flag: "--evidence-kind",
    short: "",
    help: "Filter by evidence kind",
    value_kind: ValueKind::Text,
};
const AI_PROJECT_FLAG: FlagSpec = FlagSpec {
    flag: "--project",
    short: "",
    help: "Filter by AI project",
    value_kind: ValueKind::Text,
};
const AI_SESSION_ID_FLAG: FlagSpec = FlagSpec {
    flag: "--session-id",
    short: "",
    help: "Filter by AI session id",
    value_kind: ValueKind::Text,
};
const AI_HOST_FLAG: FlagSpec = FlagSpec {
    flag: "--host",
    short: "",
    help: "Filter by hostname",
    value_kind: ValueKind::Host,
};
const AI_HOSTNAME_FLAG: FlagSpec = FlagSpec {
    flag: "--hostname",
    short: "",
    help: "Filter by hostname",
    value_kind: ValueKind::Host,
};
const AI_SINCE_FLAG: FlagSpec = FlagSpec {
    flag: "--since",
    short: "",
    help: "Start of window",
    value_kind: ValueKind::Time,
};
const AI_UNTIL_FLAG: FlagSpec = FlagSpec {
    flag: "--until",
    short: "",
    help: "End of window",
    value_kind: ValueKind::Time,
};
const AI_LIMIT_FLAG: FlagSpec = FlagSpec {
    flag: "--limit",
    short: "",
    help: "Max results",
    value_kind: ValueKind::Text,
};
const AI_WINDOW_MINUTES_FLAG: FlagSpec = FlagSpec {
    flag: "--window-minutes",
    short: "",
    help: "Incident grouping window in minutes",
    value_kind: ValueKind::Text,
};
const AI_CORRELATION_WINDOW_MINUTES_FLAG: FlagSpec = FlagSpec {
    flag: "--correlation-window-minutes",
    short: "",
    help: "Evidence correlation window in minutes",
    value_kind: ValueKind::Text,
};
const AI_SIGNAL_FLAG: FlagSpec = FlagSpec {
    flag: "--signal",
    short: "",
    help: "Require a signal type",
    value_kind: ValueKind::Text,
};
const AI_MIN_SCORE_FLAG: FlagSpec = FlagSpec {
    flag: "--min-score",
    short: "",
    help: "Minimum incident priority score",
    value_kind: ValueKind::Text,
};
const AI_INCIDENT_ID_FLAG: FlagSpec = FlagSpec {
    flag: "--incident-id",
    short: "",
    help: "Investigate a specific incident id",
    value_kind: ValueKind::Text,
};
const AI_ERROR_ONLY_FLAG: FlagSpec = FlagSpec {
    flag: "--error-only",
    short: "",
    help: "Only return failed MCP calls",
    value_kind: ValueKind::None,
};
const AI_ALL_FLAG: FlagSpec = FlagSpec {
    flag: "--all",
    short: "",
    help: "Investigate all matching incidents",
    value_kind: ValueKind::None,
};
const AI_JSON_FLAG: FlagSpec = FlagSpec {
    flag: "--json",
    short: "",
    help: "JSON output",
    value_kind: ValueKind::None,
};

pub(super) const SKILL_EVENTS_FLAGS: &[FlagSpec] = &[
    AI_SKILL_FLAG,
    AI_PLUGIN_FLAG,
    AI_TOOL_FLAG,
    AI_PROJECT_FLAG,
    AI_SESSION_ID_FLAG,
    AI_HOST_FLAG,
    AI_SINCE_FLAG,
    AI_UNTIL_FLAG,
    AI_LIMIT_FLAG,
    AI_JSON_FLAG,
];

pub(super) const SKILL_INCIDENT_FLAGS: &[FlagSpec] = &[
    AI_SKILL_FLAG,
    AI_PLUGIN_FLAG,
    AI_TOOL_FLAG,
    AI_PROJECT_FLAG,
    AI_SESSION_ID_FLAG,
    AI_HOSTNAME_FLAG,
    AI_SINCE_FLAG,
    AI_UNTIL_FLAG,
    AI_LIMIT_FLAG,
    AI_WINDOW_MINUTES_FLAG,
    AI_SIGNAL_FLAG,
    AI_MIN_SCORE_FLAG,
    AI_JSON_FLAG,
];

pub(super) const SKILL_INVESTIGATE_FLAGS: &[FlagSpec] = &[
    AI_INCIDENT_ID_FLAG,
    AI_SKILL_FLAG,
    AI_PLUGIN_FLAG,
    AI_TOOL_FLAG,
    AI_PROJECT_FLAG,
    AI_SINCE_FLAG,
    AI_UNTIL_FLAG,
    AI_LIMIT_FLAG,
    AI_WINDOW_MINUTES_FLAG,
    AI_CORRELATION_WINDOW_MINUTES_FLAG,
    AI_ALL_FLAG,
    AI_JSON_FLAG,
];

pub(super) const MCP_EVENTS_FLAGS: &[FlagSpec] = &[
    AI_TOOL_NAME_FLAG,
    AI_MCP_SERVER_FLAG,
    AI_MCP_TOOL_FLAG,
    AI_TOOL_FLAG,
    AI_PROJECT_FLAG,
    AI_SESSION_ID_FLAG,
    AI_HOST_FLAG,
    AI_ERROR_ONLY_FLAG,
    AI_SINCE_FLAG,
    AI_UNTIL_FLAG,
    AI_LIMIT_FLAG,
    AI_JSON_FLAG,
];

pub(super) const MCP_INCIDENT_FLAGS: &[FlagSpec] = &[
    AI_MCP_SERVER_FLAG,
    AI_MCP_TOOL_FLAG,
    AI_TOOL_NAME_FLAG,
    AI_TOOL_FLAG,
    AI_PROJECT_FLAG,
    AI_SESSION_ID_FLAG,
    AI_HOSTNAME_FLAG,
    AI_SINCE_FLAG,
    AI_UNTIL_FLAG,
    AI_LIMIT_FLAG,
    AI_WINDOW_MINUTES_FLAG,
    AI_SIGNAL_FLAG,
    AI_MIN_SCORE_FLAG,
    AI_JSON_FLAG,
];

pub(super) const MCP_INVESTIGATE_FLAGS: &[FlagSpec] = &[
    AI_INCIDENT_ID_FLAG,
    AI_MCP_SERVER_FLAG,
    AI_MCP_TOOL_FLAG,
    AI_TOOL_NAME_FLAG,
    AI_TOOL_FLAG,
    AI_PROJECT_FLAG,
    AI_SINCE_FLAG,
    AI_UNTIL_FLAG,
    AI_LIMIT_FLAG,
    AI_WINDOW_MINUTES_FLAG,
    AI_CORRELATION_WINDOW_MINUTES_FLAG,
    AI_ALL_FLAG,
    AI_JSON_FLAG,
];

pub(super) const HOOK_EVENTS_FLAGS: &[FlagSpec] = &[
    AI_HOOK_EVENT_FLAG,
    AI_HOOK_FLAG,
    AI_HOOK_SOURCE_FLAG,
    AI_STATUS_FLAG,
    AI_EVIDENCE_KIND_FLAG,
    AI_TOOL_FLAG,
    AI_PROJECT_FLAG,
    AI_SESSION_ID_FLAG,
    AI_HOST_FLAG,
    AI_SINCE_FLAG,
    AI_UNTIL_FLAG,
    AI_LIMIT_FLAG,
    AI_JSON_FLAG,
];

pub(super) const HOOK_INCIDENT_FLAGS: &[FlagSpec] = &[
    AI_HOOK_EVENT_FLAG,
    AI_HOOK_FLAG,
    AI_HOOK_SOURCE_FLAG,
    AI_TOOL_FLAG,
    AI_PROJECT_FLAG,
    AI_SESSION_ID_FLAG,
    AI_HOSTNAME_FLAG,
    AI_EVIDENCE_KIND_FLAG,
    AI_SINCE_FLAG,
    AI_UNTIL_FLAG,
    AI_LIMIT_FLAG,
    AI_WINDOW_MINUTES_FLAG,
    AI_SIGNAL_FLAG,
    AI_MIN_SCORE_FLAG,
    AI_JSON_FLAG,
];

pub(super) const HOOK_INVESTIGATE_FLAGS: &[FlagSpec] = &[
    AI_INCIDENT_ID_FLAG,
    AI_HOOK_EVENT_FLAG,
    AI_HOOK_FLAG,
    AI_HOOK_SOURCE_FLAG,
    AI_TOOL_FLAG,
    AI_PROJECT_FLAG,
    AI_SINCE_FLAG,
    AI_UNTIL_FLAG,
    AI_LIMIT_FLAG,
    AI_WINDOW_MINUTES_FLAG,
    AI_CORRELATION_WINDOW_MINUTES_FLAG,
    AI_ALL_FLAG,
    AI_JSON_FLAG,
];
