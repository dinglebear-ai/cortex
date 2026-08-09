/** Contract-only TypeScript declarations for Cortex Agent Observatory v1. */

export type Id = string
export type Timestamp = string
export type Sha = string
export type TraceId = string
export type SpanId = string

export type RunStatus =
  | "starting"
  | "active"
  | "waiting"
  | "idle"
  | "stale"
  | "completed"
  | "failed"
  | "abandoned"

export type TrustLevel = "verified" | "claimed" | "correlated" | "inferred" | "refuted"
export type FreshnessState = "fresh" | "delayed" | "stale" | "not_observed" | "error"
export type AgentEventKind =
  | "lifecycle"
  | "transcript"
  | "command"
  | "shell_history"
  | "git_status"
  | "git_head"
  | "git_commit"
  | "file_operation"
  | "mcp"
  | "hook"
  | "skill"
  | "llm"
  | "otlp_log"
  | "otlp_span"
  | "otlp_metric"
  | "heartbeat"
  | "error"
  | "provider_event"

export type StreamEventName =
  | "run.created"
  | "run.updated"
  | "run.status"
  | "run.event"
  | "worktree.updated"
  | "repository.updated"
  | "telemetry.updated"
  | "observatory.reset"

export interface FreshnessLane {
  state: FreshnessState
  last_observed_at: Timestamp | null
  lag_seconds: number | null
  detail: string | null
}

export interface RunFreshness {
  transcript: FreshnessLane
  command: FreshnessLane
  git: FreshnessLane
  otlp_log: FreshnessLane
  trace: FreshnessLane
  metric: FreshnessLane
  lifecycle: FreshnessLane
}

export interface Evidence {
  kind: string
  source: string
  trust: TrustLevel
  confidence: number
  first_seen_at: Timestamp
  last_seen_at: Timestamp
  detail: string | null
}

export interface RepositorySummary {
  id: Id
  key: string
  hostname: string
  primary_path: string
  name: string
  first_seen_at: Timestamp
  last_seen_at: Timestamp
  removed_at: Timestamp | null
  worktree_count: number
  active_run_count: number
}

export interface WorktreeSummary {
  id: Id
  key: string
  repository_id: Id
  hostname: string
  path: string
  branch_ref: string | null
  branch: string | null
  head_sha: Sha | null
  upstream_ref: string | null
  detached: boolean
  bare: boolean
  locked: boolean
  lock_reason: string | null
  prunable: boolean
  prune_reason: string | null
  dirty: boolean
  staged: number
  unstaged: number
  untracked: number
  ahead: number | null
  behind: number | null
  first_seen_at: Timestamp
  last_seen_at: Timestamp
  removed_at: Timestamp | null
}

export interface AgentActor {
  key: string
  native_id: string
  actor_type: string | null
  display_name: string | null
  started_at: Timestamp | null
  last_activity_at: Timestamp | null
  ended_at: Timestamp | null
}

export interface AgentRunSummary {
  id: Id
  run_key: string
  native_session_id: string
  tool: string
  provider_tool: string | null
  hostname: string
  parent_run_key: string | null
  previous_run_key: string | null
  status: RunStatus
  status_reason: string
  status_observed_at: Timestamp
  started_at: Timestamp
  last_activity_at: Timestamp
  ended_at: Timestamp | null
  transcript_path: string | null
  primary_worktree: WorktreeSummary | null
  primary_branch: string | null
  start_head_sha: Sha | null
  current_head_sha: Sha | null
  event_count: number
  error_count: number
  freshness: RunFreshness
}

export interface AgentRunDetail {
  run: AgentRunSummary
  actors: AgentActor[]
  worktree_evidence: Evidence[]
  available_event_kinds: AgentEventKind[]
  commit_summary: GitCommitSummary | null
  latest_stream_cursor: Id
}

export interface AgentRunEvent<TPayload = unknown> {
  id: Id
  event_key: string
  run_key: string
  actor_key: string | null
  worktree_id: Id | null
  commit_sha: Sha | null
  observed_at: Timestamp
  ingested_at: Timestamp
  kind: AgentEventKind
  source_kind: string
  source_id: string
  source_log_id: Id | null
  provider_sequence: number | null
  trace_id: TraceId | null
  span_id: SpanId | null
  severity: string
  title: string
  summary: string
  payload: TPayload | null
  content_scrubbed: boolean
}

export interface GitCommitSummary {
  sha: Sha
  parent_shas: Sha[]
  author_name: string | null
  authored_at: Timestamp | null
  committed_at: Timestamp | null
  subject: string
  changed_files: number | null
  insertions: number | null
  deletions: number | null
  changed_paths: string[]
  first_observed_at: Timestamp
  last_observed_at: Timestamp
  reachable: boolean
  evidence: Evidence[]
}

export interface SpanSummary {
  trace_id: TraceId
  span_id: SpanId
  parent_span_id: SpanId | null
  name: string
  kind: number
  start_time: Timestamp
  end_time: Timestamp
  duration_nano: number
  status_code: number
  status_message: string | null
  service_name: string | null
  attributes: Record<string, unknown>
}

export interface MetricPoint {
  point_key: string
  metric_name: string
  description: string
  unit: string
  instrument_kind: "gauge" | "sum" | "histogram" | "exponential_histogram" | "summary"
  start_time: Timestamp | null
  time: Timestamp
  value: unknown
  attributes: Record<string, unknown>
  exemplars: Record<string, unknown>[]
}

export interface Pagination {
  limit: number
  next_cursor: string | null
  truncated: boolean
}

export interface PageEnvelope {
  pagination: Pagination
  as_of: Timestamp
  stream_cursor: Id
}

export interface RepositoryPage extends PageEnvelope {
  repositories: RepositorySummary[]
}

export interface WorktreePage extends PageEnvelope {
  worktrees: WorktreeSummary[]
}

export interface AgentRunPage extends PageEnvelope {
  runs: AgentRunSummary[]
}

export interface AgentRunEventPage extends PageEnvelope {
  run_key: string
  events: AgentRunEvent[]
}

export interface AgentRunTelemetry {
  run_key: string
  spans: SpanSummary[]
  metrics: MetricPoint[]
  summary: Record<string, unknown>
  freshness: RunFreshness
  span_pagination: Pagination
  metric_pagination: Pagination
  as_of: Timestamp
}

export interface StreamEnvelope<TData extends Record<string, unknown> = Record<string, unknown>> {
  id: Id
  event: StreamEventName
  entity_type: string
  entity_key: string
  run_key: string | null
  occurred_at: Timestamp
  data: TData
}

export interface ProjectionCursorStatus {
  source_name: string
  last_source_id: Id
  source_max_id: Id
  lag_rows: number
  last_success_at: Timestamp | null
  last_error_at: Timestamp | null
  last_error: string | null
  retry_count: number
}

export interface ObservatoryStatus {
  enabled: boolean
  schema_version: number
  projection_version: number
  projector: {
    running: boolean
    cursors: ProjectionCursorStatus[]
  }
  git_observer: Record<string, unknown>
  stream: Record<string, unknown>
  otlp: Record<string, unknown>
  web: {
    source_revision: string
    aurora_revision: string
    next_version: string
    [key: string]: unknown
  }
  warnings: string[]
}

export interface ApiError {
  error: string
  message: string
  request_id?: string | null
  details?: Record<string, unknown>
}

export function lengthPrefixed(parts: readonly string[]): string {
  return parts.map((part) => `${new TextEncoder().encode(part).byteLength}:${part}`).join("|")
}

export function createRunKey(host: string, tool: string, session: string): string {
  const normalizedHost = host.trim()
  const normalizedTool = tool.trim().toLowerCase()
  const normalizedSession = session.trim()
  if (!normalizedHost || !normalizedTool || !normalizedSession) {
    throw new TypeError("host, tool, and session must be non-empty")
  }
  return `v1|${lengthPrefixed([normalizedHost, normalizedTool, normalizedSession])}`
}

export function isDecimalId(value: string): value is Id {
  return /^[0-9]+$/.test(value)
}

export function isRunStatus(value: string): value is RunStatus {
  return [
    "starting",
    "active",
    "waiting",
    "idle",
    "stale",
    "completed",
    "failed",
    "abandoned",
  ].includes(value)
}
