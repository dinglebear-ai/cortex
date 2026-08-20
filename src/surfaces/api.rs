use super::*;

pub(super) const API_SURFACE_SPECS: &[SurfaceSpec] = &[
    api!("/api/search", Search, Canonical, Read),
    api!("/api/filter", Search, Canonical, Read),
    api!("/api/feed", Search, Canonical, Read),
    api!("/api/tail", Search, Canonical, Read),
    api!("/api/errors", Analysis, RetainedProtocolCompatibility, Read),
    api!("/api/hosts", Hosts, Canonical, Read),
    api!(
        "/api/correlate",
        Correlate,
        RetainedProtocolCompatibility,
        Read
    ),
    api!("/api/stats", Stats, Canonical, Read),
    api!("/api/version", Runtime, Canonical, Info),
    api!(
        "/api/source-ips",
        Hosts,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/timeline",
        Analysis,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/patterns",
        Analysis,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/ingest-rate",
        Stats,
        RetainedProtocolCompatibility,
        Read
    ),
    api!("/api/get", Search, Canonical, Read),
    api!(
        "/api/host-state",
        State,
        RetainedProtocolCompatibility,
        Read
    ),
    api!("/api/context", Search, Canonical, Read),
    api!(
        "/api/fleet-state",
        State,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/correlate-state",
        Correlate,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/topic-correlate",
        Correlate,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/errors/unaddressed",
        Alerts,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/errors/ack",
        Alerts,
        RetainedProtocolCompatibility,
        Admin
    ),
    api!(
        "/api/errors/unack",
        Alerts,
        RetainedProtocolCompatibility,
        Admin
    ),
    api!(
        "/api/notifications/recent",
        Alerts,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/notifications/test",
        Alerts,
        RetainedProtocolCompatibility,
        Admin
    ),
    api!(
        "/api/file-tails",
        Ingest,
        RetainedProtocolCompatibility,
        Admin
    ),
    api!(
        "/api/silent-hosts",
        Hosts,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/clock-skew",
        State,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/anomalies",
        Analysis,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/compare",
        Analysis,
        RetainedProtocolCompatibility,
        Read
    ),
    api!("/api/apps", Search, Canonical, Read),
    // GET is the canonical read surface. POST on the same route requires the
    // existing explicit admin-token guard in api.rs.
    api!("/api/artifact-evidence", Evidence, Canonical, Read),
    api!(
        "/api/similar-incidents",
        Analysis,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/incident-context",
        Analysis,
        RetainedProtocolCompatibility,
        Read
    ),
    api!("/api/graph/entity", Graph, Canonical, Read),
    api!("/api/graph/around", Graph, Canonical, Read),
    api!("/api/graph/explain", Graph, Canonical, Read),
    api!("/api/graph/evidence", Graph, Canonical, Read),
    api!("/api/sessions/incidents", Sessions, Canonical, Read),
    api!("/api/sessions/investigate", Sessions, Canonical, Read),
    api!("/api/sessions/llm-invocations", Sessions, Canonical, Admin),
    api!("/api/sessions/skills", Sessions, Canonical, Read),
    api!("/api/sessions/skill-incidents", Sessions, Canonical, Read),
    api!("/api/sessions/skill-investigate", Sessions, Canonical, Read),
    api!("/api/sessions/mcp-events", Sessions, Canonical, Read),
    api!("/api/sessions/mcp-incidents", Sessions, Canonical, Read),
    api!("/api/sessions/mcp-investigate", Sessions, Canonical, Read),
    api!("/api/sessions/hooks", Sessions, Canonical, Read),
    api!("/api/sessions/hook-incidents", Sessions, Canonical, Read),
    api!("/api/sessions/hook-investigate", Sessions, Canonical, Read),
    api!(
        "/api/compose/status",
        Compose,
        RetainedProtocolCompatibility,
        Read
    ),
    api!(
        "/api/compose/doctor",
        Compose,
        RetainedProtocolCompatibility,
        Read
    ),
    api!("/api/sessions", Sessions, Canonical, Read),
    api!("/api/sessions/search", Sessions, Canonical, Read),
    api!("/api/sessions/abuse", Sessions, Canonical, Read),
    api!("/api/sessions/correlate", Sessions, Canonical, Read),
    api!("/api/sessions/blocks", Sessions, Canonical, Read),
    api!("/api/sessions/context", Sessions, Canonical, Read),
    api!("/api/sessions/tools", Sessions, Canonical, Read),
    api!("/api/sessions/projects", Sessions, Canonical, Read),
    api!("/api/sessions/checkpoints", Sessions, Canonical, Read),
    api!("/api/sessions/errors", Sessions, Canonical, Read),
    api!(
        "/api/sessions/prune-checkpoints",
        Sessions,
        Canonical,
        Admin
    ),
    api!("/api/db/status", Db, RetainedTopLevelOperational, Read),
    api!("/api/db/integrity", Db, RetainedTopLevelOperational, Read),
    api!(
        "/api/db/integrity/background",
        Db,
        RetainedTopLevelOperational,
        Admin
    ),
    api!(
        "/api/db/integrity/jobs/{id}",
        Db,
        RetainedTopLevelOperational,
        Read
    ),
    api!("/api/db/checkpoint", Db, RetainedTopLevelOperational, Admin),
    api!("/api/db/vacuum", Db, RetainedTopLevelOperational, Admin),
    api!("/api/db/backup", Db, RetainedTopLevelOperational, Admin),
    api!("/api/ai", Sessions, RemovedCleanBreak, Read, replace: "/api/sessions", reason: "AI session REST routes moved to /api/sessions with no compatibility shim"),
    api!("/api/ai/search", Sessions, RemovedCleanBreak, Read, replace: "/api/sessions/search", reason: "AI session REST routes moved to /api/sessions with no compatibility shim"),
    api!("/api/ai/abuse", Sessions, RemovedCleanBreak, Read, replace: "/api/sessions/abuse", reason: "AI session REST routes moved to /api/sessions with no compatibility shim"),
    api!("/api/ai/correlate", Sessions, RemovedCleanBreak, Read, replace: "/api/sessions/correlate", reason: "AI session REST routes moved to /api/sessions with no compatibility shim"),
    api!("/api/ai/blocks", Sessions, RemovedCleanBreak, Read, replace: "/api/sessions/blocks", reason: "AI session REST routes moved to /api/sessions with no compatibility shim"),
    api!("/api/ai/context", Sessions, RemovedCleanBreak, Read, replace: "/api/sessions/context", reason: "AI session REST routes moved to /api/sessions with no compatibility shim"),
    api!("/api/ai/tools", Sessions, RemovedCleanBreak, Read, replace: "/api/sessions/tools", reason: "AI session REST routes moved to /api/sessions with no compatibility shim"),
    api!("/api/ai/projects", Sessions, RemovedCleanBreak, Read, replace: "/api/sessions/projects", reason: "AI session REST routes moved to /api/sessions with no compatibility shim"),
];
