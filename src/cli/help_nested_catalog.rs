//! Help entries for nested command paths.
use super::NestedCommandDoc;

pub(super) const NESTED_CATALOG: &[NestedCommandDoc] = &[
    NestedCommandDoc {
        path: "ingest inventory",
        summary: "Collect native homelab inventory into the private filesystem cache",
        usage: &[
            "cortex ingest inventory refresh [--json]",
            "cortex ingest inventory status [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions page",
        summary: "Read the next rendered transcript page for one session",
        usage: &[
            "cortex sessions page --project PATH --tool TOOL --session-id ID --host HOST [--cursor CURSOR] [--limit N] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions search",
        summary: "Full-text search over indexed AI transcript sessions",
        usage: &[
            "cortex sessions search QUERY [--project PATH] [--tool TOOL] [--since TIME] [--until TIME] [--limit N] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions abuse",
        summary: "Find risky or failure-related transcript messages",
        usage: &[
            "cortex sessions abuse [--project PATH] [--tool TOOL] [--since TIME] [--until TIME] [--limit N] [--before N] [--after N] [--term WORD] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions incidents",
        summary: "Cluster AI transcript abuse matches into incidents",
        usage: &[
            "cortex sessions incidents [--project PATH] [--tool TOOL] [--since TIME] [--until TIME] [--limit N] [--window-minutes N] [--term WORD] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions investigate",
        summary: "Expand AI incidents into evidence bundles",
        usage: &[
            "cortex sessions investigate [--project PATH] [--tool TOOL] [--since TIME] [--until TIME] [--limit N] [--window-minutes N] [--correlation-window-minutes N] [--term WORD] [--detail compact|full] [--include-transcript] [--max-bytes N] [--json]",
            "Default output is compact; use --detail full for complete evidence.",
        ],
    },
    NestedCommandDoc {
        path: "sessions assess",
        summary: "Assess one AI incident with optional model context",
        usage: &[
            "cortex sessions assess INCIDENT_ID [--model MODEL] [--project PATH] [--tool TOOL] [--since TIME] [--until TIME] [--limit N] [--window-minutes N] [--correlation-window-minutes N] [--term WORD] [--dry-run] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions correlate",
        summary: "Correlate AI transcript anchors with non-AI logs",
        usage: &[
            "cortex sessions correlate [--project PATH] [--tool TOOL] [--session-id ID] [--ai-query FTS] [--log-query FTS] [--host HOST] [--source SOURCE] [--app APP] [--since TIME] [--until TIME] [--window-minutes N] [--severity-min LEVEL] [--limit N] [--events-per-anchor N] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions blocks",
        summary: "AI transcript activity grouped into 5-hour UTC blocks",
        usage: &[
            "cortex sessions blocks [--project PATH] [--tool TOOL] [--since TIME] [--until TIME] [--limit N] [--detail compact|full] [--json]",
            "Default output is capped for interactive use; use --detail full for every block.",
        ],
    },
    NestedCommandDoc {
        path: "sessions context",
        summary: "Recent AI transcript context for one project",
        usage: &["cortex sessions context PROJECT [--tool TOOL] [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions tools",
        summary: "List AI tools present in transcript metadata",
        usage: &["cortex sessions tools [--project PATH] [--since TIME] [--until TIME] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions projects",
        summary: "List AI projects present in transcript metadata",
        usage: &["cortex sessions projects [--tool TOOL] [--since TIME] [--until TIME] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions index",
        summary: "Index local AI transcript roots",
        usage: &["cortex sessions index [--path PATH] [--since TIME] [--force] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions add",
        summary: "Index one AI transcript file",
        usage: &["cortex sessions add FILE [--force] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions watch",
        summary: "Run the local transcript watch daemon",
        usage: &[
            "cortex sessions watch [--path PATH] [--debounce-ms N] [--settle-ms N] [--max-retries N] [--no-initial-scan] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions checkpoints",
        summary: "List AI transcript indexing checkpoints",
        usage: &["cortex sessions checkpoints [--errors] [--missing] [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions errors",
        summary: "List AI transcript parse errors",
        usage: &["cortex sessions errors [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions prunecheckpoints",
        summary: "Prune stale AI indexing checkpoints",
        usage: &["cortex sessions prunecheckpoints [--dry-run] [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions doctor",
        summary: "Check local AI transcript indexing prerequisites",
        usage: &["cortex sessions doctor [--strict-permissions] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions watchstatus",
        summary: "Inspect the local AI transcript watch service",
        usage: &["cortex sessions watchstatus [--json]"],
    },
    NestedCommandDoc {
        path: "sessions smokewatch",
        summary: "Run a local AI transcript watch smoke test",
        usage: &["cortex sessions smokewatch [--json]"],
    },
    NestedCommandDoc {
        path: "sessions similar",
        summary: "Find incidents similar to a free-text query",
        usage: &[
            "cortex sessions similar QUERY [--host HOST] [--app APP] [--severity-min LEVEL] [--since TIME] [--until TIME] [--window-minutes N] [--limit N] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions incidentcontext",
        summary: "Build incident context from a recent or explicit time window",
        usage: &[
            "cortex sessions incidentcontext [QUERY] [--since TIME] [--until TIME] [--host HOST] [--app APP] [--severity-min LEVEL] [--limit N] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions llminvocations",
        summary: "Inspect recent LLM invocation audit records",
        usage: &[
            "cortex sessions llminvocations [--since TIME] [--action ACTION] [--status STATUS] [--limit N] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions skillincidents",
        summary: "List skill-usage incidents",
        usage: &[
            "cortex sessions skillincidents [SKILL] [--since TIME] [--until TIME] [--limit N] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions skillinvestigate",
        summary: "Investigate incidents for one skill",
        usage: &["cortex sessions skillinvestigate SKILL [--incident-id ID] [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions skillassess",
        summary: "Assess one skill with deterministic evidence and optional LLM synthesis",
        usage: &["cortex sessions skillassess SKILL [--no-llm] [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions mcpevents",
        summary: "List or backfill MCP tool-call events",
        usage: &[
            "cortex sessions mcpevents [--since TIME] [--limit N] [--json]",
            "cortex sessions mcpevents backfill [--since TIME] [--limit N] [--dry-run] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions mcpincidents",
        summary: "List MCP tool-usage incidents",
        usage: &["cortex sessions mcpincidents [SERVER] [--since TIME] [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions mcpinvestigate",
        summary: "Investigate incidents for one MCP server or tool",
        usage: &["cortex sessions mcpinvestigate TARGET [--incident-id ID] [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions mcpassess",
        summary: "Assess one MCP server or tool with optional LLM synthesis",
        usage: &["cortex sessions mcpassess TARGET [--no-llm] [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions hookevents",
        summary: "List collected hook execution events",
        usage: &[
            "cortex sessions hookevents [--hook NAME] [--status STATUS] [--since TIME] [--limit N] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "sessions hookincidents",
        summary: "List hook execution incidents",
        usage: &["cortex sessions hookincidents [HOOK] [--since TIME] [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions hookinvestigate",
        summary: "Investigate incidents for one hook",
        usage: &["cortex sessions hookinvestigate HOOK [--incident-id ID] [--limit N] [--json]"],
    },
    NestedCommandDoc {
        path: "sessions hooksbackfill",
        summary: "Backfill hook events from transcripts",
        usage: &["cortex sessions hooksbackfill [--since TIME] [--limit N] [--dry-run] [--json]"],
    },
    NestedCommandDoc {
        path: "db backup",
        summary: "Create a WAL-safe SQLite backup",
        usage: &["cortex db backup [PATH] [--json]"],
    },
    NestedCommandDoc {
        path: "db status",
        summary: "Inspect SQLite maintenance state",
        usage: &["cortex db status [--check-coord] [--json]"],
    },
    NestedCommandDoc {
        path: "db integrity",
        summary: "Run SQLite integrity checks",
        usage: &[
            "cortex db integrity [--quick] [--background] [--json]",
            "cortex db integrity status JOB_ID [--json]",
        ],
    },
    NestedCommandDoc {
        path: "db checkpoint",
        summary: "Run a SQLite WAL checkpoint",
        usage: &["cortex db checkpoint [passive|full|restart|truncate] [--json]"],
    },
    NestedCommandDoc {
        path: "db vacuum",
        summary: "Run SQLite incremental or full vacuum",
        usage: &["cortex db vacuum [--pages N] [--full] [--force] [--json]"],
    },
    NestedCommandDoc {
        path: "compose status",
        summary: "Inspect the resolved Docker Compose runtime",
        usage: &[
            "cortex compose status [--compose-file PATH] [--project-dir DIR] [--project-name NAME] [--service NAME] [--container NAME] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "compose doctor",
        summary: "Diagnose Docker Compose/listener ownership",
        usage: &[
            "cortex compose doctor [--compose-file PATH] [--project-dir DIR] [--project-name NAME] [--service NAME] [--container NAME] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "compose pull",
        summary: "Pull the resolved Docker Compose image",
        usage: &["cortex compose pull [--dry-run] [--allow-cwd-target] [--json]"],
    },
    NestedCommandDoc {
        path: "compose up",
        summary: "Recreate the resolved Docker Compose service",
        usage: &["cortex compose up [--dry-run] [--allow-cwd-target] [--json]"],
    },
    NestedCommandDoc {
        path: "compose restart",
        summary: "Restart the resolved Docker Compose service",
        usage: &["cortex compose restart [--dry-run] [--allow-cwd-target] [--json]"],
    },
    NestedCommandDoc {
        path: "compose down",
        summary: "Stop the resolved Docker Compose service",
        usage: &["cortex compose down --yes [--dry-run] [--allow-cwd-target] [--json]"],
    },
    NestedCommandDoc {
        path: "compose logs",
        summary: "Show bounded stack logs or one service's logs",
        usage: &[
            "cortex compose logs [--tail N] [--json]",
            "cortex compose logs SERVICE [--since TIME] [--until TIME] [--tail N] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "setup start",
        summary: "Choose a role, configure clients and collection, and verify a managed installation",
        usage: &[
            "cortex setup start [--role server|client|agent] [--server URL] [--token-file PATH] [--api-token-file PATH] [--clients codex,claude,gemini] [--capabilities LIST] [--set KEY=VALUE] [--secret-file KEY=PATH] [--backup-schedule|--no-backup-schedule] [--dry-run] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "setup effective",
        summary: "Show redacted effective setup configuration with provenance",
        usage: &["cortex setup effective [--json]"],
    },
    NestedCommandDoc {
        path: "setup verify",
        summary: "Verify readiness, MCP authentication and REST access",
        usage: &[
            "cortex setup verify [--server URL] [--token-file PATH] [--api-token-file PATH] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "setup backup",
        summary: "Create, schedule or restore verified managed snapshots",
        usage: &[
            "cortex setup backup create [--home PATH]",
            "cortex setup backup schedule install|check|remove [--home PATH]",
            "cortex setup backup restore|rollback STAMP --yes [--home PATH]",
        ],
    },
    NestedCommandDoc {
        path: "setup check",
        summary: "Audit managed setup without changing files",
        usage: &["cortex setup check [--json]"],
    },
    NestedCommandDoc {
        path: "setup repair",
        summary: "Repair managed setup idempotently",
        usage: &["cortex setup repair [--json]"],
    },
    NestedCommandDoc {
        path: "setup install",
        summary: "Install plugin setup artifacts",
        usage: &["cortex setup install [--json]"],
    },
    NestedCommandDoc {
        path: "setup pluginhook",
        summary: "Run plugin setup hook repair or audit mode",
        usage: &["cortex setup pluginhook [--no-repair] [--json]"],
    },
    NestedCommandDoc {
        path: "setup deploy",
        summary: "Provision cortex locally or on remote hosts",
        usage: &[
            "cortex setup deploy preflight [--json]",
            "cortex setup deploy local [--dry-run] [--json]",
            "cortex setup deploy remote [--home PATH] HOST [--dry-run] [--json]",
            "cortex setup deploy agent [--hosts h1,h2] [--target URL] [--heartbeat-token-file PATH] [--docker] [--journald] [--binary PATH]",
        ],
    },
    NestedCommandDoc {
        path: "update",
        summary: "Update the configured server and host-agent clients",
        usage: &[
            "cortex update [all|server|clients|agents] [--dry-run] [--json]",
            "cortex update server [--dry-run] [--json]",
            "cortex update clients [--dry-run] [--json] [--binary PATH]",
            "cortex update agents [--dry-run] [--json] [--binary PATH]",
            "cortex update config server --host HOST --home PATH [--json]",
            "cortex update config clients --hosts h1,h2 [--target URL] [--docker] [--journald] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "setup doctor",
        summary: "Run setup diagnostics across all phases",
        usage: &["cortex setup doctor [--json]"],
    },
    NestedCommandDoc {
        path: "alerts signatures",
        summary: "List error signatures",
        usage: &[
            "cortex alerts signatures [--include-acknowledged] [--limit N] [--json]",
            "cortex alerts signatures ack HASH [--notes TEXT] [--json]",
            "cortex alerts signatures unack HASH [--reason TEXT] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "alerts notifications",
        summary: "List recent notification firings",
        usage: &[
            "cortex alerts notifications [--rule-id ID] [--since TIME] [--limit N] [--json]",
            "cortex alerts notifications test [--body TEXT] [--json]   (requires --http)",
        ],
    },
    NestedCommandDoc {
        path: "ingest shell user",
        summary: "Index shell history typed by a human",
        usage: &[
            "cortex ingest shell user index PATH [--shell zsh] [--json]",
            "cortex ingest shell user atuinindex [PATH] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "ingest shell agent",
        summary: "Ingest AI-agent-issued shell commands",
        usage: &[
            "cortex ingest shell agent index PATH [--json] [--server URL] [--token TOKEN]",
            "cortex ingest shell agent wrap SPOOL -- COMMAND...",
        ],
    },
    NestedCommandDoc {
        path: "heartbeat agent",
        summary: "Run the host heartbeat agent",
        usage: &[
            "cortex heartbeat agent [--target URL] [--token TOKEN] [--interval-secs N] [--probe-deadline-ms N] [--collection-deadline-ms N] [--retry-buffer N] [--host-id-path PATH] [--once|--emit] [--json]",
        ],
    },
    NestedCommandDoc {
        path: "ingest filetail",
        summary: "List configured file-tail sources",
        usage: &[
            "cortex ingest filetail list [--json]",
            "cortex ingest filetail status [--json]",
            "cortex ingest filetail add PATH [--id ID] [--tag TAG] [--host HOST] [--facility FACILITY] [--severity SEVERITY] [--from-start] [--json]",
            "cortex ingest filetail remove ID [--json]",
            "cortex ingest filetail enable ID [--json]",
            "cortex ingest filetail disable ID [--json]",
        ],
    },
];
