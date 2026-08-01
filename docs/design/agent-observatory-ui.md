# Agent Observatory Next.js and Aurora UI design

Status: proposed
Aurora source revision: `19e87c6b26dd32eb3be3db4d8b6fb32c99cd1d6d`
Next.js floor: `16.2.11`

## Product posture

The observatory is an operations workspace, not a marketing dashboard. It must support sustained use, dense evidence, rapid keyboard navigation, and mobile inspection without turning live activity into visual noise.

The primary mental model is:

```text
repository -> worktree/branch -> agent run -> chronological evidence
```

The UI must never present inferred worktree or commit attribution with the same visual certainty as verified evidence.

## Route map

| Route | Purpose | Static export rule |
| --- | --- | --- |
| `/app/` | Cortex application launcher and connection state | static page |
| `/app/agents/` | repository and agent observatory | static page, client data |
| `/app/investigate/` | migrated current investigation graph and Ask workflow | static page, client data |

Selections and filters live in URL search parameters so a view can be copied without including the bearer token:

```text
/app/agents/?repository=12&worktree=44&run=<encoded-run-key>&tab=timeline&kind=command&kind=git_commit
```

The app must restore the selected repository/worktree/run from the URL after the operator re-enters a token. It must not fetch sensitive data before authentication.

## Application shell

Desktop uses a three-column resizable workspace:

```text
┌ repository/worktree navigator ┬ run list ┬ selected run workspace ┐
│ 280px default                 │ 360px    │ flexible               │
└───────────────────────────────┴──────────┴─────────────────────────┘
```

Aurora components:

- `aurora-sidebar-block` for global navigation and connection controls
- `aurora-resizable-panels` for the three workspace regions
- `aurora-breadcrumb` for repository/worktree/run hierarchy
- `aurora-command-palette` for keyboard search and navigation
- `aurora-tooltip` for compact icon actions
- `aurora-toast` for recoverable operation feedback
- `aurora-banner` for stale/reset/degraded global state

Mobile uses one content stack with a top app bar:

- repository/worktree navigation opens in `aurora-sheet`
- run list opens in a second sheet or becomes the current screen
- the selected run workspace is the primary route content
- browser back follows selection history
- no horizontal three-panel squeeze

## Authentication state

### Disconnected

The launcher and each data route show one Aurora connection card with:

- bearer token input
- Connect button
- explanation that token remains in memory
- server URL fixed to same origin in production
- no saved-token checkbox

Components:

- Aurora input, button, card, field/label, alert, and password visibility primitives from the resolved `aurora-base` graph

### Connecting

- disable duplicate submit
- show `aurora-spinner`
- do not mount live stream until initial snapshots succeed

### Unauthorized

- clear token from memory
- stop all fetches and stream retries
- show `aurora-banner` with reconnect action
- retain only non-sensitive URL selection

### Connected

The connection control collapses to server revision, ingest status, and a Clear token action. The token value is never rendered back into the DOM after submission.

## Repository/worktree navigator

The left navigator groups repositories by host, then worktrees under each repository.

Repository row:

- display name
- host
- active run count
- worktree count
- last Git reconcile freshness

Worktree row:

- branch or Detached HEAD
- dirty indicator and change counts
- short HEAD
- ahead/behind
- active run count
- lock/prunable/removed status

Components:

- Aurora tree/sidebar primitives
- `aurora-status-indicator`
- Aurora badge and tooltip
- `aurora-empty-state`
- `aurora-skeleton`
- Aurora scroll area

Behavior:

- repository and worktree nodes are keyboard navigable
- expanded state is UI-local and may persist only in memory
- selecting a worktree updates URL and run-list filter
- removed worktrees are hidden by default and exposed through a filter
- inferred repository mappings show a trust badge

## Run list

The middle panel is a virtualized list or compact table of runs sorted by activity.

Each row contains:

- tool and optional actor count
- status and status reason
- branch/worktree
- host
- relative last activity
- error count
- signal freshness mini-strip

Components:

- `aurora-data-table` in wide mode
- Aurora compact list/card composition in narrow mode
- `aurora-filter-bar`
- `aurora-status-indicator`
- Aurora badge, avatar/icon, skeleton, empty state, pagination

Filters:

- repository/worktree
- status multi-select
- tool multi-select
- host
- branch
- active only
- time range
- text search

The filter bar serializes non-secret values into the URL. The UI debounces search input but sends exact server filters. It never performs a misleading partial client-only search over one loaded page.

## Selected run header

The header must make identity and confidence obvious:

- tool and native session ID
- status plus reason
- host
- repository/worktree/branch
- start and current HEAD
- started/last activity/ended
- transcript path, subject to privacy setting
- verified/inferred trust indicator
- signal freshness
- Copy run key action

Components:

- `aurora-ai-agent`
- `aurora-ai-branch`
- `aurora-ai-commit`
- `aurora-status-indicator`
- Aurora badges, breadcrumbs, buttons, tooltip, separator

If worktree attribution is ambiguous, the header says “Multiple possible worktrees” and opens an evidence sheet. It never silently chooses a weak relation.

## Run workspace tabs

Use Aurora tabs with the following routes/facets:

### Timeline

A unified virtualized timeline containing all event kinds.

- `aurora-timeline` provides semantic item structure
- `@tanstack/react-virtual` controls mounted rows
- event renderers use Aurora AI message, tool, task, plan, branch, commit, terminal, badge, code, and alert components
- older pages load above without scroll jump
- new live events append below
- when following live, viewport remains at bottom
- when operator scrolls away, append count appears on a Return to live button

Event renderer mapping:

| Event | Aurora rendering |
| --- | --- |
| transcript user/assistant | `aurora-ai-conversation`, `aurora-ai-message` |
| provider plan/task | `aurora-ai-plan`, `aurora-ai-task` |
| command/shell history | `aurora-terminal-block` |
| MCP/tool | `aurora-ai-tool`, `aurora-tool-calls` |
| branch/head | `aurora-ai-branch` |
| commit | `aurora-ai-commit` |
| hook/skill/lifecycle/error | `aurora-timeline`, status, badge, alert |
| OTLP span/metric | timeline summary plus telemetry deep link |

### Transcript

Transcript-only reading view:

- conversation grouping by actor
- timestamp and provider metadata toggle
- full-text search constrained to run
- jump to match
- optional tool-call folding
- no Markdown HTML injection; render safe text/code through reviewed components

Components: Aurora AI conversation/message/tool/plan/task, search input, code block, accordion/collapsible, empty state.

### Commands

Chronological command table and detail drawer:

- command, cwd, start, duration, exit, actor, source
- secret-scrubbed indicator
- filter failed/long-running/Git commands
- selected command detail uses terminal block

Components: Aurora data table, terminal, sheet, badge, filter bar.

### Git

Repository state and exact commit evidence:

- current worktree card
- branch/HEAD transitions
- exact attributed commits
- confidence and evidence
- changed-path summaries
- detached/rebase/reset observations

Components: Aurora branch, commit, timeline, data table, code workspace for changed path tree, alert for ambiguous attribution.

Full source diffs are not shown unless a later opt-in API is designed.

### Telemetry

Two views:

- span waterfall/tree for one or all traces
- metric cards and time-series summaries

Components:

- `aurora-chart`
- Aurora tree/table/status/card primitives
- resizable detail panel
- trace span detail sheet

The initial span waterfall may be a feature-specific SVG or CSS layout wrapped in Aurora cards and controls. It must use Aurora tokens and controls; no new chart dependency is permitted until the performance/interaction test proves Aurora/Recharts cannot satisfy the view.

### MCP, hooks, and skills

Combined activity workspace with facets:

- MCP server/tool calls and results
- hooks by event, command, exit, duration
- skill loads and incidents
- LLM invocation metadata

Components: Aurora tool calls, AI tool, timeline, data table, alert, tabs.

### Evidence

Raw provenance and trust view:

- all run/worktree and run/commit evidence
- source table and source ID
- confidence
- observed interval
- redacted bounded metadata

Components: Aurora data table, code block, badges, tooltip, sheet.

## Repository overview state

When a repository is selected without a run, the main workspace shows:

- active runs by worktree
- activity sparkline
- dirty worktrees
- recent exact commits
- stale or failed sessions
- Git observer health

Use Aurora chart, cards, data table, branch, commit, status, and empty-state components.

## No-selection state

Show an Aurora empty state explaining the navigation model and keyboard shortcut. Do not fill the page with fake telemetry.

## Live stream state machine

Client states:

`disconnected -> loading_snapshot -> connecting_stream -> live -> reconnecting -> reset_required -> loading_snapshot`

Terminal state: `unauthorized`.

Rules:

1. initial repository/run snapshots return `stream_cursor`
2. connect after that cursor
3. apply stream events through a reducer keyed by event ID
4. ignore duplicate or older IDs
5. on reset, stop stream, preserve selection, refetch snapshots, reconnect
6. on network failure, exponential backoff with jitter and visible reconnect banner
7. after five consecutive failures, keep retrying at bounded interval and expose manual retry
8. page visibility changes may reduce nonessential refetches but must keep cursor correctness

Use `eventsource-parser` with fetch response chunks. The client aborts the fetch on token clear, route disposal, or filter change.

## State ownership

- bearer token: React context, memory only
- route selection/filter: URL search parameters
- server snapshots and stream reducer: route-level React reducer and hooks
- panel sizes, expanded tree, active tab: component memory
- no Redux/Zustand/query dependency in first release
- all fetch wrappers require an explicit token argument and return typed results or `ApiError`

## Aurora registry installation

Production installation command pattern:

```bash
AURORA_SHA=19e87c6b26dd32eb3be3db4d8b6fb32c99cd1d6d
pnpm dlx shadcn@4.13.1 add   "https://raw.githubusercontent.com/dinglebear-ai/aurora/$AURORA_SHA/public/r/aurora-base.json"   ...
```

The actual command is generated from `web/aurora.lock.json`. Installation must occur into a clean staging directory, then the resulting diff is reviewed and copied. The implementation agent must never run a mutable `@latest` shadcn or mutable Aurora registry URL for the committed production source.

## Top-level Aurora coverage inventory

| UI need | Top-level registry item |
| --- | --- |
| tokens, base styles, shared primitives | `aurora-base` |
| shell navigation | `aurora-sidebar-block` |
| desktop workspace | `aurora-resizable-panels` |
| repository/run/command/evidence tables | `aurora-data-table` |
| filtering | `aurora-filter-bar` |
| unified event structure | `aurora-timeline` |
| metric and activity graphs | `aurora-chart` |
| status/freshness/trust | `aurora-status-indicator` |
| log detail | `aurora-log-viewer` |
| commands | `aurora-terminal-block` |
| MCP calls | `aurora-tool-calls` |
| changed-path workspace | `aurora-code-workspace` |
| transcripts | `aurora-ai-conversation`, `aurora-ai-message` |
| provider and actor identity | `aurora-ai-agent` |
| plans/tasks/tools | `aurora-ai-plan`, `aurora-ai-task`, `aurora-ai-tool` |
| Git evidence | `aurora-ai-branch`, `aurora-ai-commit` |
| route and error states | resolved Aurora dialog/sheet/tabs/banner/toast/empty/skeleton/spinner set |

The component inventory test scans feature imports and JSX symbols. Any rendered bespoke primitive requires either replacing it with Aurora or documenting why no registry component exists and adding an Aurora upstream issue.

## Responsive breakpoints

- under 768 px: single pane plus sheets
- 768-1199 px: navigator sheet, run list + detail split
- 1200 px and above: full three-pane workspace

Breakpoints follow Tailwind defaults unless Aurora defines a stronger semantic token. Tests cover Pixel-class mobile, tablet landscape, 1440 desktop, and 1920 desktop.

## Keyboard contract

- `Cmd/Ctrl+K`: command palette
- `g r`: focus repositories
- `g s`: focus sessions
- `g t`: focus timeline
- `j/k`: next/previous item within the focused list, when not editing text
- `Enter`: select
- `Escape`: close sheet/dialog or return focus
- `Shift+L`: return to live timeline
- `/`: focus search for current view

Shortcuts must not fire from editable fields and must be shown in tooltips/help.

## Accessibility contract

- semantic landmarks and headings per route
- tree/list/table roles only when component behavior satisfies the role
- visible focus rings from Aurora tokens
- status text plus icon, never color alone
- timestamp `time` elements with machine-readable datetime
- relative timestamps have exact tooltip/accessible label
- live events use one rate-limited polite announcement, not one announcement per row
- virtual rows maintain stable accessible names and keyboard selection
- sheets/dialogs trap focus and restore trigger focus
- charts provide table/text alternatives
- reduced-motion disables animated transitions and live pulses

## Performance implementation

- dynamically import telemetry charts, Cytoscape investigation graph, and code workspace
- virtualize run and timeline lists when thresholds are exceeded
- request event summaries first; lazy-load payload on expansion
- batch stream reducer updates in one animation frame
- memoize event renderers by event ID
- avoid context values that change on every stream event
- use route-level code splitting from the App Router static export
- enforce compressed JS and CSS budgets in CI

## Visual and interaction test matrix

Required screenshots and interaction tests:

1. disconnected desktop/mobile
2. connected empty inventory
3. repository overview
4. active Claude run with transcript/tool/Git events
5. Codex run with traces and metrics
6. Gemini run with rewritten transcript and telemetry
7. ambiguous worktree attribution
8. stream reconnect banner
9. stream reset and snapshot recovery
10. unauthorized token expiry
11. long 10,000-event timeline with bounded DOM
12. dark and light mode if both are shipped
13. 200% browser zoom
14. reduced motion
15. keyboard-only critical workflow

Screenshot tests must mask timestamps/cursors or use deterministic fixtures. Visual approval is necessary but never substitutes for semantic assertions.

## Build outputs

The frontend build must produce:

- static routes and chunks in `web/out`
- vendored Aurora fonts
- no source maps
- `cortex-assets.json` with asset/CSP metadata
- bundle analyzer summary
- `aurora.lock.json` audit report
- Playwright screenshots and traces on failure

The real Rust server must be used in the final E2E gate so asset routing, bearer API, SSE, CSP, and deep-link fallback are tested together.
