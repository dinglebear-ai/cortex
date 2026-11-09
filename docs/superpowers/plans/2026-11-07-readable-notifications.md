# Readable Cortex notifications

Goal: Cortex push messages show readable text, useful fields, and a small status emoji. JSON payloads and normalized signature noise stay in Cortex.

Use the existing Apprise client as the final formatting boundary so all delivery paths, including persisted outbox messages and test notifications, receive the same protection. Send text format to avoid HTML entity and Markdown rendering problems. Keep routing, severity, deduplication, acknowledgement, retry behavior, and database evidence unchanged. Use the existing Rust toolchain and dependencies.

- [x] Add failing HTTP payload tests for embedded/nested JSON, malformed JSON, escaped text, Unicode bounds, and unsafe mentions. Add a scanner regression using the collector failure.
- [x] Add a bounded text formatter that turns structured fields into labels, prioritizes the cause, omits telemetry bookkeeping, and never falls back to raw JSON. Make the Apprise client send text.
- [x] Convert recurring alerts, built-in alert rules, digests, and direct health/test notifications into readable text with light emoji. Preserve the original stored evidence and rule behavior.
- [x] Run focused tests, the workspace suite, formatting, Clippy, documentation checks, and self-review. Record deployment/publication status separately.

Review focus: nested JSON encoded inside strings; truncated structured logs; previously escaped outbox content; long Unicode messages; punctuation that must remain literal in text-mode delivery. Exercise these through the real local HTTP payload, without sending messages to a real Gotify destination.

Validation: 112 focused notification/scanner/health checks passed with four test threads. Formatting, all-target/all-feature Clippy, repository pre-push checks, documentation checks, and the notification harness selftest passed. Documentation tests completed with zero executable cases.

The full workspace run was not green: four metadata/discovery failures also reproduce on unchanged main at 1a92e07c. Two additional transcript-scanner cases were interrupted after running for over six minutes. The diagnostic run reported 3,803 passes, four assertion failures, two interrupted tests, and four skips. Those results are not release signoff.

Independent review found a multiline JSON leak; the regression reproduced it, and the fix passed the final focused checks and follow-up review. Publication and deployment remain separate from this local change.

PR review also reproduced lost error causes for numeric-leading bracketed IPv6 destinations and timestamps. Array detection now checks numeric token syntax so these remain literal. Structured logs are parsed before each HTML entity-decoding pass, preserving quoted string values and legacy escaped documents. Delivery regressions preserve both the problem and cause; follow-up independent source review found no related blocker.
