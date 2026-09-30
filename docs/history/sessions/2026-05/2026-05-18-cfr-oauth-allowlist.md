---
date: 2026-05-18 00:42:27 EDT
repo: https://github.com/jmagar/syslog-mcp
branch: work/cfr-oauth-allowlist
starting head: f35357a
plan: /home/jmagar/workspace/syslog-mcp/06-all-issues.md
working directory: /home/jmagar/workspace/syslog-mcp/.worktrees/cfr-oauth-allowlist
worktree: /home/jmagar/workspace/syslog-mcp/.worktrees/cfr-oauth-allowlist
pr: "#31 fix: reject unsupported OAuth allowed_emails https://github.com/jmagar/syslog-mcp/pull/31"
---

# CFR OAuth Allowlist Session

## User Request

Use the `work-it` flow for Agent 1's CFR-001, CFR-002, CFR-003, and CFR-009 assignment: make the OAuth `allowed_emails` contract fail closed or enforce it, update tests, correct docs, verify, push, and open a PR from an isolated worktree.

## Session Overview

- Created `.worktrees/cfr-oauth-allowlist` on branch `work/cfr-oauth-allowlist`.
- Changed OAuth startup validation to reject non-empty config-level `allowed_emails` because syslog-mcp does not pass that TOML field into lab-auth.
- Updated runtime construction so `RuntimeCore::for_server` validates the same OAuth invariants as `Config::load`, while `mcp.no_auth=true` still short-circuits ignored OAuth fields into `LoopbackDev`.
- Kept lab-auth DB `allowed_users` behavior accurate in operator docs and made OAuth mode use lab-auth's headless `bearer_only_router` so `/register` and `/auth/login` remain 404.
- Bumped the repo version to `0.25.4`, added a changelog entry, pushed the branch, and opened PR #31.

## Sequence of Events

1. Inspected the main checkout state and read `/home/jmagar/workspace/syslog-mcp/06-all-issues.md`.
2. Created or reused the requested isolated worktree and confirmed branch `work/cfr-oauth-allowlist`.
3. Located the mismatch across `src/config.rs`, `src/runtime.rs`, `src/config_tests.rs`, and OAuth docs.
4. Patched config validation, runtime validation, tests, route mounting, docs, and version metadata.
5. Ran focused tests first, then full test/lint/version gates.
6. Pushed `work/cfr-oauth-allowlist`, opened PR #31, checked for PR comments, and saved this session note.
7. Ran additional review waves. Follow-up findings corrected runtime-level validation, stale `/register`/`/auth/login` route comments, `allowed_users` operator docs, refresh-token revocation SQL, and `no_auth` override ordering.

## Key Findings

- lab-auth does not consume syslog-mcp's TOML `allowed_emails`, but it does enforce `admin_email` plus DB-backed `allowed_users` rows.
- `src/config.rs` now detects any non-blank config-level `allowed_emails` entry and rejects it before accepting OAuth startup, except when `mcp.no_auth=true` makes auth config unused.
- `src/runtime.rs` calls `validate_auth_config` during `RuntimeCore::for_server`, so programmatic configs get the same fail-closed behavior before DB initialization.
- OAuth mode now mounts lab-auth's headless `bearer_only_router`, keeping `/register` and `/auth/login` out of the syslog-mcp route surface.
- GitHub PR comments had no actionable line comments; CodeRabbit posted only a rate-limit notice.

## Technical Decisions

- Chose fail-closed startup rejection for config-level `allowed_emails` rather than pretending those TOML entries are enforced by lab-auth.
- Rejected any non-empty `allowed_emails` in OAuth mode, not just `allowed_emails` without `admin_email`, so operators cannot believe additional config emails are enforced when they are ignored.
- Preserved lab-auth DB `allowed_users` as active runtime behavior and documented it instead of claiming V1 has no DB allowlist.
- Preserved `mcp.no_auth=true` as an auth-policy override: stale OAuth fields are ignored when the runtime is explicitly in `LoopbackDev`.
- Kept `allowed_emails` parsing intact for schema compatibility and future support.
- Used `RUSTC_WRAPPER=` for verification after the first focused test hit a local sccache allocation failure.

## Files Modified

- `src/config.rs`: reject unsupported OAuth `allowed_emails`, require `admin_email`, and let `no_auth` bypass ignored auth fields.
- `src/config_tests.rs`: replace the old accepting test with fail-closed cases, update valid OAuth fixtures to use `admin_email`, and cover stale OAuth fields under `no_auth`.
- `src/runtime.rs`: validate auth config in runtime construction, align the lab-auth bridge comment, and disable dynamic registration.
- `src/runtime_tests.rs`: cover runtime-level rejection before DB initialization and `no_auth` bypass behavior.
- `src/mcp/routes.rs`, `src/mcp/routes_tests.rs`, `tests/auth_modes.rs`: mount the headless OAuth router and assert `/register` and `/auth/login` are 404 in all modes.
- `docs/OAUTH.md`: document config-level `admin_email`, active lab-auth `allowed_users`, subject-based refresh-token cleanup, and disabled dynamic registration.
- `docs/contracts/config-schema.md`: make the normative schema reject config-level `allowed_emails` in OAuth mode, except when `no_auth=true`.
- `docs/contracts/runtime-lifecycle.md`: update startup invariants.
- `docs/contracts/credentials.md`: remove the unsupported env allowlist row.
- `docs/SETUP.md`: update the OAuth setup summary wording.
- `Cargo.toml`, `Cargo.lock`, `server.json`, `CHANGELOG.md`: bump to `0.25.4` and record the fix.

## Commands Executed

| Command | Result |
|---|---|
| `git worktree add -b work/cfr-oauth-allowlist .worktrees/cfr-oauth-allowlist HEAD` | Existing worktree/branch found and reused. |
| `RUSTC_WRAPPER= cargo test oauth_mode --lib` | Passed: focused OAuth config tests after using the correct filter. |
| `RUSTC_WRAPPER= cargo test oauth` | Passed: OAuth-filtered unit and integration tests. |
| `RUSTC_WRAPPER= cargo test no_auth_ignores_stale_oauth_fields` | Passed: config and runtime no-auth override tests. |
| `RUSTC_WRAPPER= cargo test register_returns_404_in_all_modes` | Passed: route and integration assertions. |
| `RUSTC_WRAPPER= cargo test auth_login` | Passed: route and integration assertions. |
| `cargo fmt --check` | Passed after applying `cargo fmt`. |
| `scripts/check-version-sync.sh` | Passed: all checked version files at `0.25.4`. |
| `RUSTC_WRAPPER= cargo clippy -- -D warnings` | Passed. |
| `RUSTC_WRAPPER= cargo test` | Passed: full test suite and doctests. |
| `git push -u origin HEAD` | Passed for the initial PR push; a follow-up push updates PR #31 after this note. |
| `gh pr view/checks` | No actionable PR comments; prior checks were green on the previously pushed commit. |

## Errors Encountered

- Initial `cargo test config_tests::oauth_mode --lib` selected zero tests because the sidecar module path did not match the filter.
- The first focused test compile failed under `/usr/bin/sccache` with `failed to zip up compiler outputs` and `Allocation error : not enough memory`; rerunning with `RUSTC_WRAPPER=` avoided sccache and passed.
- One combined `cargo test` command was invalid because Cargo accepts one name filter; reran the targeted filters separately.
- Parallel Cargo test invocations contended on build locks but completed successfully.

## Behavior Changes

| Before | After |
|---|---|
| OAuth startup accepted config-level `allowed_emails` as satisfying the allowlist requirement. | OAuth startup rejects non-empty config-level `allowed_emails` until syslog-mcp can pass or enforce it. |
| Programmatic `RuntimeCore::for_server` could bypass config validation. | Runtime construction validates auth config before DB initialization. |
| `no_auth=true` could still fail on stale OAuth fields after runtime validation was added. | `no_auth=true` bypasses ignored auth fields and returns `LoopbackDev`. |
| Route comments implied `/register` and `/auth/login` might be mounted in OAuth mode. | syslog-mcp mounts lab-auth's headless router and tests both paths as 404 in all modes. |
| Docs implied config `allowed_emails` and SIGHUP/restart were the user-management path. | Docs distinguish config `allowed_emails` from active lab-auth DB `allowed_users`, and state config changes are restart-only. |
| Tests locked in unsupported `allowed_emails`-only success. | Tests lock in fail-closed rejection for unsupported config-level `allowed_emails`. |

## Verification Evidence

| Command | Expected | Actual | Status |
|---|---|---|---|
| `RUSTC_WRAPPER= cargo test oauth_mode --lib` | OAuth config tests pass | Focused tests passed | Pass |
| `RUSTC_WRAPPER= cargo test oauth` | OAuth-related tests pass | 21 lib tests plus relevant integration filters passed | Pass |
| `RUSTC_WRAPPER= cargo test no_auth_ignores_stale_oauth_fields` | no-auth override stays `LoopbackDev` | 2 tests passed | Pass |
| `RUSTC_WRAPPER= cargo test register_returns_404_in_all_modes` | DCR route stays unmounted | 2 tests passed | Pass |
| `RUSTC_WRAPPER= cargo test auth_login` | browser login route stays unmounted | 2 tests passed | Pass |
| `cargo fmt --check` | Formatting clean | No diff | Pass |
| `scripts/check-version-sync.sh` | Version files aligned | all 2 checked files at v0.25.4 | Pass |
| `RUSTC_WRAPPER= cargo clippy -- -D warnings` | No warnings | Finished successfully | Pass |
| `RUSTC_WRAPPER= cargo test` | Full suite green | 584 lib, 48 bin, integration tests, and doctests passed | Pass |

## Risks and Rollback

- Risk: existing operators with config-level `allowed_emails` set under OAuth will now receive a startup config error. This is intentional fail-closed behavior for an unenforced config field.
- Risk: lab-auth DB `allowed_users` rows remain active. This is now documented explicitly for revocation and inventory procedures.
- Rollback: revert PR #31 or remove config-level `allowed_emails` from config and set `admin_email` or lab-auth DB `allowed_users` to the intended OAuth users.

## Decisions Not Taken

- Did not implement local callback allowlist enforcement because lab-auth owns OAuth callback/session issuance and current runtime code does not expose an enforced config-level `allowed_emails` bridge.
- Did not remove the `allowed_emails` field from `AuthConfig` because preserving parse compatibility keeps future multi-user enforcement migration simpler.

## References

- Issue register: `/home/jmagar/workspace/syslog-mcp/06-all-issues.md`.
- PR: https://github.com/jmagar/syslog-mcp/pull/31.
- CodeRabbit PR comment: rate limit notice only; no actionable review comments.

## Open Questions

- Whether future syslog-mcp support should map config-level `allowed_emails` into lab-auth, rely on lab-auth DB `allowed_users`, or support both with clear precedence.

## Next Steps

- Push the follow-up review fixes to PR #31.
- Watch CI/review after the follow-up push.
