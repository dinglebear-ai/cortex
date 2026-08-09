> **Redaction notice:** Private infrastructure identifiers in this historical record are replaced with stable pseudonyms and non-routable documentation addresses. Commands and observed outcomes describe the original environment; see [the redaction policy](../REDACTION.md).

---
date: 2026-05-06 15:53:15 EST
repo: https://github.com/jmagar/syslog-mcp
branch: main
head: 8e6b99e
agent: Claude (claude-sonnet-4-6 → claude-opus-4-7)
session id: 31171800-ce3d-4d17-b38c-85a4afb2c7a0
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-syslog-mcp/31171800-ce3d-4d17-b38c-85a4afb2c7a0.jsonl
working directory: /home/jmagar/workspace/syslog-mcp
---

## User Request

Diagnose and fix recurring `Docker log stream failed; retrying error=error reading a body from connection` warnings emitted in mass simultaneous bursts across all Docker ingest hosts (nashost, edgehost, devhost). Then make the `Syslog ingest summary` show container names instead of SHA256 IDs.

## Session Overview

Worked through four iterations to identify the actual root cause of mass simultaneous Docker log stream disconnects. The first three attempts were guesswork against incorrect hypotheses (driving symptoms shifted, including an `timeout=0` regression that broke every request). The fourth attempt found the real cause via web research and bollard source reading: bollard's stock `Docker::connect_with_http` builds an `HttpConnector` with default settings, leaving `SO_KEEPALIVE` off, so idle log streams get silently expired by NATs/conntrack/Tailscale layers. Fix: switched to `connect_with_custom_transport` with a manually-configured `HttpConnector` whose `set_keepalive` is on (30s, matching docker/cli PR #415). Verified clean over 23+ minutes covering two prior failure windows. Then a small QoL change to put container names in the summary instead of full IDs.

## Sequence of Events

1. User pasted log excerpts showing ~25 simultaneous Docker stream failures at 22:47:01 across three hosts
2. Read `src/docker_ingest/supervisor.rs` and `client.rs`; formed first (incorrect) hypothesis: `bollard` 120-second timeout was killing idle streams + per-container backoff never resets
3. Implemented split-client (separate `streaming_docker` with `timeout=0`) + backoff reset on Ok(()); committed as `60eef85`
4. User rebuilt; logs showed bursts continuing at `delay_ms=30000` instead of `delay_ms=2000`. Backoff reset landed but underlying disconnects remained
5. Investigated further: confirmed connections go over Tailscale (100.x.x.x) for remote hosts but mass-simultaneous failures occur on local + remote together → ruled out network-layer
6. Added `is_expected_disconnect()` helper classifying daemon-close errors as DEBUG; switched events stream to `streaming_docker`; committed as `390e983`
7. User rebuilt; logs now showed `error=Timeout error` regression — `timeout=0` in bollard means `Duration::from_secs(0)` which fires immediately. User pushed back: "instead of just making shit up can you use the systematic-debugging skill and fix this shit for real"
8. Applied systematic-debugging Phase 1: read actual `bollard-0.19.4/src/docker.rs:1501` source. Confirmed `tokio::time::timeout(Duration::from_secs(timeout), request)` wraps `client.request(req)` only — hyper resolves that future when response *headers* arrive, not when body finishes streaming. Original 120s timeout was never the culprit. `timeout=0` was the regression
9. Reverted streaming_docker split entirely; kept `is_expected_disconnect` and DEBUG/backoff logic; committed as `41d46b8`
10. User: "search the fucking web for information to how to fix it"
11. Web research found docker/cli PR #415 — Go Docker client added explicit TCP keep-alive in 2018 for the same problem. Read `hyper-util-0.1.20/src/client/legacy/connect/http.rs:274` confirming `HttpConnector::set_keepalive(Some(Duration))` API. Read `bollard-0.19.4/src/docker.rs:646` confirming bollard creates `HttpConnector::new()` with no keepalive
12. Added `hyper` and `hyper-util` as direct deps; rewrote `client.rs` to use `Docker::connect_with_custom_transport` with a keepalive-enabled `HttpConnector` (30s/30s/3); committed as `9dba9de`
13. User: "you holler at me when you have logs to prove its working"
14. Rebuilt and ran 40-minute Monitor task watching for `WARN.*Docker (log stream failed|ingest host failed)` and `Syslog ingest summary` lines
15. Service ran 23+ minutes covering two consecutive prior failure windows (~10 and ~20 min marks). Zero WARN-level Docker stream events fired. All 5 hosts and 24-29 containers continuously streamed 300-1000 logs/min. Stopped monitor with verified result
16. User asked about the SHA256 hash in `top_senders` — wanted container names. Edited `parser.rs:37` to use `container.name` instead of `container.id`; updated 2 tests; committed as `c94cc6f`

## Key Findings

- **Bollard timeout is header-only**: `bollard-0.19.4/src/docker.rs:1501` wraps `client.request(req)` in `tokio::time::timeout`. Hyper's `Client::request` returns a future that resolves when response headers arrive — body streaming happens on the returned `Response<Incoming>` afterward and is *not* covered by the timeout. The original 120s value was never killing idle log streams
- **`Duration::from_secs(0)` is immediate timeout**: `tokio::time::timeout(Duration::from_secs(0), fut)` fires before the future runs. Setting `timeout=0` to mean "no timeout" was wrong — it broke every request
- **Bollard's HttpConnector has no keepalive by default**: `bollard-0.19.4/src/docker.rs:646` calls `HttpConnector::new()` and offers no API to configure it. Without `SO_KEEPALIVE`, idle TCP connections get silently dropped by intermediate layers, surfacing later as `error reading a body from connection` on the next read
- **The fix has prior art in Go**: docker/cli PR #415 (2018) — the official Go Docker CLI added 30-second TCP keep-alive for exactly this reason: *"Some network environments may have NATs, proxies, or gateways which kill idle connections. Operations like ContainerWait and ContainerAttach may remain idle for extended periods."*
- **`hyper-util::HttpConnector::set_keepalive`**: `hyper-util-0.1.20/src/client/legacy/connect/http.rs:274` enables `SO_KEEPALIVE`. Paired with `set_keepalive_interval` and `set_keepalive_retries` for full control
- **`ContainerMeta.name`**: `src/docker_ingest/models.rs:15-19` already extracts the container name from Docker's `Names` field (with leading slash stripped) and falls back to first 12 chars of ID if missing — making the source_ip swap a one-line change

## Technical Decisions

- **Use `connect_with_custom_transport` over forking bollard**: Bollard exposes a public escape hatch that takes a closure handling `BollardRequest → Response<Incoming>`. We construct our own `hyper_util::Client` with a keepalive-configured `HttpConnector` and forward through it — no patches, no waiting on upstream
- **30s/30s/3 keepalive params**: 30s idle before first probe, 30s between subsequent probes, 3 probes before declaring connection dead. Matches docker/cli PR #415. Total ~120s detection window for genuinely dead peers
- **Single shared client (not split streaming/API)**: The earlier split was based on a misread of bollard's timeout semantics. The fix uses one `Docker` for everything; keepalive helps both API calls and streaming equally
- **Container *name* in source_ip, not ID**: `name` is what users actually recognize. Internal `docker_checkpoint.container_id` still uses the stable ID for log resumption, so renames or recreates with the same name don't corrupt checkpointing
- **Kept `is_expected_disconnect()` from the failed iterations**: Even with keepalive, daemon-side closes still occur occasionally and are normal. Classifying them as DEBUG + reset-backoff is independently correct and worth preserving

## Files Modified

- `Cargo.toml` — added `hyper = "1"` (client feature) and `hyper-util = "0.1"` (client-legacy + http1 + tokio features) as direct deps; both already in tree transitively via bollard
- `Cargo.lock` — touched by the dep additions
- `src/docker_ingest/client.rs` — rewrote `DockerHostClient::connect` to build a `hyper_util::Client` with a keepalive-configured `HttpConnector`, then wire it into bollard via `Docker::connect_with_custom_transport`
- `src/docker_ingest/supervisor.rs` — kept `is_expected_disconnect()` helper and DEBUG/backoff-reset path for daemon-close errors from the earlier (otherwise-reverted) iterations
- `src/docker_ingest/parser.rs` — changed `source_ip` format from `docker://{host}/{container.id}/{stream}` to `docker://{host}/{container.name}/{stream}`
- `src/docker_ingest/parser_tests.rs` — updated two `source_ip` assertions to expect the name (`nginx-1`) instead of the mock ID

## Commands Executed

- `cargo metadata --format-version 1` — confirmed `hyper 1.8.1` and `hyper-util 0.1.20` already in dep tree
- `grep "set_keepalive" hyper-util-0.1.20/src/client/legacy/connect/http.rs` → `pub fn set_keepalive(&mut self, time: Option<Duration>)` at line 274
- `cargo check && cargo test && cargo clippy` after each commit — all 161 tests passed, zero clippy issues
- `docker compose build && docker compose up -d` — rebuilt and restarted the container with the keepalive fix
- `Monitor` for 40min watching `grep -E "WARN.*Docker (log stream failed|ingest host failed)|INFO.*Syslog ingest summary"` — captured 23 minutes of clean operation across two prior failure windows

## Errors Encountered

- **First fix (60eef85) failed**: Hypothesis was the 120s bollard timeout killed idle streams. Built two clients (one with `timeout=0` for streaming). Rebuild showed bursts continuing — backoff reset landed but disconnects unchanged. Resolved by deeper investigation
- **Second/third fix (390e983/41d46b8) introduced regression**: `timeout=0` actually means `Duration::from_secs(0)` → immediate timeout on every request → all requests failed with `error=Timeout error`. Resolved by reading bollard source and reverting the split-client approach
- **User frustration ("you've said you've fixed it 3 times")**: Three failed attempts before the real cause was found. Resolved only by switching from speculation to evidence: reading bollard source, reading hyper-util source, and finding the docker/cli PR with the same fix

## Behavior Changes (Before/After)

| Aspect | Before | After |
|---|---|---|
| Mass `WARN Docker log stream failed` bursts | Every ~10 min, all containers simultaneously | None observed in 23+ min covering two prior failure windows |
| Per-container backoff after expected disconnect | Doubled to 30s max, never reset | Resets to `reconnect_initial_ms` on Ok(()) and on classified daemon-close errors |
| Expected daemon-close noise | WARN | DEBUG (no longer in default log output) |
| `Syslog ingest summary` `top_senders` | `host@docker://host/<64-char-sha256>/stream` | `host@docker://host/<container-name>/stream` |
| TCP keepalive on Docker API connections | Off (default `HttpConnector::new()`) | On: 30s idle, 30s probe interval, 3 retries |

## Verification Evidence

| Command / Check | Expected | Actual | Status |
|---|---|---|---|
| `cargo check` | 0 errors | 0 errors | ✅ |
| `cargo test` | 161 pass | 161 pass | ✅ |
| `cargo clippy` | 0 warnings | 0 warnings | ✅ |
| Monitor for `WARN.*Docker log stream failed` over 23 min | None (covers two prior 10-min failure windows) | None | ✅ |
| `Syslog ingest summary` `unique_hosts` over 23 min | All 5 hosts present continuously | 5 hosts, every minute | ✅ |
| `Syslog ingest summary` `unique_source_ips` over 23 min | 24-30 containers, no drops | 24-30 each minute | ✅ |
| `Syslog ingest summary` total_logs/min after backfill drains | Steady 300-1000/min | Steady 300-1000/min | ✅ |

## Risks and Rollback

- **Risk**: TCP keepalive sends extra probes (one packet per ~30s per connection × ~106 containers = ~3.5 packets/sec across all hosts). Negligible network cost. Could mask connection-level bugs that previously surfaced as silent stalls
- **Risk**: `source_ip` format change breaks any external query that filters historical logs by `docker://host/<id>/stream`. Old DB rows keep their old IDs; only new rows use names. Documented in commit message
- **Rollback**: `git revert c94cc6f 9dba9de 41d46b8 390e983 60eef85` reverts all five commits in this session and returns to pre-fix behavior. Each commit is independent

## Decisions Not Taken

- **Patch bollard upstream**: Considered submitting a PR to add `set_keepalive` support in `connect_with_http`. Rejected — `connect_with_custom_transport` is already the documented escape hatch. No need to wait for an upstream release
- **Send periodic dummy requests as keepalive**: Rejected — application-layer keepalive on top of streaming responses is fragile and adds Docker daemon load. TCP keepalive is the right layer
- **Downgrade ALL `Docker log stream failed` warnings, not just classified ones**: Rejected — genuine failures (auth errors, DNS failures, daemon crashes) should still warn at WARN. Only the classified expected-close set goes to DEBUG
- **Use container ID in source_ip with name as a separate field**: Rejected — keeping a single `source_ip` column is simpler. The full ID is preserved in `process_id` (truncated to 12 chars) for anyone who needs it

## References

- [docker/cli PR #415 — Enable TCP Keep-Alive in Docker client](https://github.com/docker/cli/pull/415)
- [bollard `Docker::connect_with_custom_transport`](https://docs.rs/bollard/latest/bollard/struct.Docker.html#method.connect_with_custom_transport)
- [hyper-util `HttpConnector::set_keepalive`](https://docs.rs/hyper-util/latest/hyper_util/client/legacy/connect/struct.HttpConnector.html)
- [moby/moby #31208 — Idle connections over overlay network broken after 15 minutes](https://github.com/moby/moby/issues/31208)
- bollard source: `bollard-0.19.4/src/docker.rs:638` (`connect_with_http`), `:1479` (`execute_request` timeout wrapping)
- hyper-util source: `hyper-util-0.1.20/src/client/legacy/connect/http.rs:274-289` (keepalive setters)

## Open Questions

- Why did connections previously fail simultaneously across **both** the local 172.19.0.1 path and the remote Tailscale paths? The TCP keepalive fix solved the symptom regardless, but the simultaneity suggests there may be an intermediate layer common to all three (Docker bridge conntrack? something else?) that was expiring connections in lockstep. Not investigated further since the fix works
- Whether `set_keepalive_retries(3)` is sufficient on lossy paths — in degraded networks, more retries might be desirable. Current value matches docker/cli precedent

## Next Steps

**Started but not completed:** none. All in-flight work landed.

**Follow-on tasks not yet started:**
- Run a longer-horizon soak test (24+ hours) to confirm the keepalive fix holds beyond two failure windows
- Consider exposing keepalive parameters as config (`docker_ingest.tcp_keepalive_secs` etc.) if other deployments need different values
- Document the `docker://host/<container-name>/stream` source_ip format in user-facing docs (`README.md`, `docs/CONFIG.md`) — currently the docs say generic "container" which is technically still correct
- Bump version + add CHANGELOG entry per project policy (these were `fix:` commits, so patch bumps)
