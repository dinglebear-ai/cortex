#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
validator="$repo_root/scripts/validate-transcript-forward-env-rename.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

git -C "$tmp" init -q
mkdir -p "$tmp/scripts" "$tmp/src" "$tmp/docs/contracts"
cp "$validator" "$tmp/scripts/validate-transcript-forward-env-rename.sh"
cat > "$tmp/src/heartbeat_agent.rs" <<'EOF'
pub const AI_TRANSCRIPT_FORWARD_ENV: &str = "CORTEX_AGENT_AI_TRANSCRIPT_FORWARD";
pub const AI_TRANSCRIPT_FORWARD_LEGACY_ENV: &str = "CORTEX_AGENT_AI_TRANSCRIPTS";
EOF
cat > "$tmp/docs/contracts/agent-observatory.md" <<'EOF'
Current: `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD`; deprecated compatibility alias: `CORTEX_AGENT_AI_TRANSCRIPTS`.
EOF
git -C "$tmp" add .
(cd "$tmp" && bash scripts/validate-transcript-forward-env-rename.sh)

cat > "$tmp/systemd-unit" <<'EOF'
Environment=CORTEX_AGENT_AI_TRANSCRIPTS=true
EOF
git -C "$tmp" add systemd-unit
if (cd "$tmp" && bash scripts/validate-transcript-forward-env-rename.sh >/dev/null 2>&1); then
    echo "validator accepted a deprecated key in an extensionless systemd file" >&2
    exit 1
fi

rm "$tmp/systemd-unit"
git -C "$tmp" add -u
cat > "$tmp/src/heartbeat_agent.rs" <<'EOF'
pub const AI_TRANSCRIPT_FORWARD_ENV: &str = "CORTEX_AGENT_AI_TRANSCRIPT_FORWARD";
// CORTEX_AGENT_AI_TRANSCRIPTS is convenient shorthand.
EOF
git -C "$tmp" add src/heartbeat_agent.rs
if (cd "$tmp" && bash scripts/validate-transcript-forward-env-rename.sh >/dev/null 2>&1); then
    echo "validator accepted an unapproved occurrence inside an allowlisted file" >&2
    exit 1
fi

cat > "$tmp/src/heartbeat_agent.rs" <<'EOF'
pub const AI_TRANSCRIPT_FORWARD_ENV: &str = "CORTEX_AGENT_AI_TRANSCRIPT_FORWARD";
pub const AI_TRANSCRIPT_FORWARD_LEGACY_ENV: &str = "CORTEX_AGENT_AI_TRANSCRIPTS";
EOF
cat > "$tmp/docs/contracts/agent-observatory.md" <<'EOF'
This configured example must fail:
```ini
CORTEX_AGENT_AI_TRANSCRIPTS=true
```
EOF
git -C "$tmp" add src/heartbeat_agent.rs docs/contracts/agent-observatory.md
if (cd "$tmp" && bash scripts/validate-transcript-forward-env-rename.sh >/dev/null 2>&1); then
    echo "validator accepted executable legacy config in an allowlisted doc" >&2
    exit 1
fi

echo "transcript-forward rename validator negative fixtures passed"
