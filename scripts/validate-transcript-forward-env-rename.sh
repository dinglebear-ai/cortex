#!/usr/bin/env bash
# ENV-004: Strict occurrence allowlist for deprecated CORTEX_AGENT_AI_TRANSCRIPTS
#
# This script validates that the legacy environment variable name only appears
# in approved locations as part of the transcript-forwarding env rename track.
#
# Approved allowlist locations:
#   - src/heartbeat_agent.rs (compatibility resolver)
#   - src/heartbeat_agent_tests.rs (compatibility tests)
#   - src/setup/doctor.rs (doctor migration)
#   - src/setup/doctor_tests.rs (migration tests)
#   - src/agent_deploy_tests.rs (deployment regression tests)
#   - src/setup/heartbeat_agent.rs (setup generation - tests only)
#   - src/setup/heartbeat_agent_tests.rs (setup tests)
#   - docs/plans/agent-observatory/01a-transcript-forward-env-rename.md (plan doc)
#   - docs/plans/2026-07-31-agent-observatory-implementation.md (implementation plan)
#   - docs/plans/agent-observatory/06-production-hardening-and-docs.md (release gate plan)
#   - docs/plans/agent-observatory/proof/PROOF.md (verification ledger)
#   - docs/contracts/agent-observatory.md (contract - documented as deprecated)
#   - docs/research/2026-07-31-agent-observatory.md (research doc)
#   - docs/specs/agent-observatory.md (spec doc)
#   - CHANGELOG.md (deprecation entry when published)
#   - This validation script

set -euo pipefail

LEGACY_VAR="CORTEX_AGENT_AI_TRANSCRIPTS"
NEW_VAR="CORTEX_AGENT_AI_TRANSCRIPT_FORWARD"

# Allowlist of files that may reference the legacy variable
declare -A ALLOWLIST
ALLOWLIST["src/heartbeat_agent.rs"]=1
ALLOWLIST["src/heartbeat_agent_tests.rs"]=1
ALLOWLIST["src/setup/doctor.rs"]=1
ALLOWLIST["src/setup/doctor_tests.rs"]=1
ALLOWLIST["src/agent_deploy_tests.rs"]=1
ALLOWLIST["src/setup/heartbeat_agent.rs"]=1
ALLOWLIST["src/setup/heartbeat_agent_tests.rs"]=1
ALLOWLIST["docs/plans/agent-observatory/01a-transcript-forward-env-rename.md"]=1
ALLOWLIST["docs/plans/2026-07-31-agent-observatory-implementation.md"]=1
ALLOWLIST["docs/plans/agent-observatory/06-production-hardening-and-docs.md"]=1
ALLOWLIST["docs/plans/agent-observatory/proof/PROOF.md"]=1
ALLOWLIST["docs/contracts/agent-observatory.md"]=1
ALLOWLIST["docs/research/2026-07-31-agent-observatory.md"]=1
ALLOWLIST["docs/specs/agent-observatory.md"]=1
ALLOWLIST["CHANGELOG.md"]=1
ALLOWLIST["scripts/validate-transcript-forward-env-rename.sh"]=1

echo "Validating strict occurrence allowlist for $LEGACY_VAR..."

FAIL=0
FOUND_IN_ALLOWLIST=0
VIOLATIONS=()

# Search for occurrences of the legacy variable
while IFS= read -r -d '' file; do
    # Skip if in allowlist
    if [[ -v "ALLOWLIST[$file]" ]]; then
        FOUND_IN_ALLOWLIST=$((FOUND_IN_ALLOWLIST + 1))
        continue
    fi

    # Count occurrences
    COUNT=$(grep -c "$LEGACY_VAR" "$file" || true)
    if [[ $COUNT -gt 0 ]]; then
        VIOLATIONS+=("$file:$COUNT occurrences")
        FAIL=1
    fi
done < <(
    git grep -l -z "$LEGACY_VAR" -- \
        '*.rs' '*.md' '*.yml' '*.yaml' '*.sh' '*.json' '*.toml' '*.txt' \
        2>/dev/null || true
)

if [[ $FAIL -eq 1 ]]; then
    echo "❌ FAIL: Found $LEGACY_VAR outside allowlist"
    for violation in "${VIOLATIONS[@]}"; do
        echo "  - $violation"
    done
    echo ""
    echo "The legacy variable must only appear in these approved locations:"
    for file in "${!ALLOWLIST[@]}"; do
        echo "  - $file"
    done
    exit 1
fi

echo "✅ PASS: All $LEGACY_VAR occurrences are within allowlist"
echo "   Found in $FOUND_IN_ALLOWLIST allowlist files"

# Verify NEW variable is present in key locations
echo ""
echo "Verifying $NEW_VAR is properly documented..."

# Check contract documents for new variable
if grep -q "$NEW_VAR" docs/contracts/agent-observatory.md; then
    echo "✅ Contract documents $NEW_VAR"
else
    echo "❌ FAIL: Contract missing $NEW_VAR documentation"
    FAIL=1
fi

# Check that new variable appears in heartbeat_agent.rs
if grep -q "$NEW_VAR" src/heartbeat_agent.rs; then
    echo "✅ heartbeat_agent.rs uses $NEW_VAR"
else
    echo "❌ FAIL: heartbeat_agent.rs missing $NEW_VAR"
    FAIL=1
fi

# Check that .env.example uses only new name
if [[ -f .env.example ]]; then
    if grep -q "$LEGACY_VAR" .env.example; then
        echo "❌ FAIL: .env.example contains legacy $LEGACY_VAR"
        FAIL=1
    else
        echo "✅ .env.example uses only $NEW_VAR or neither"
    fi
fi

if [[ $FAIL -eq 1 ]]; then
    exit 1
fi

echo ""
echo "🎉 All transcript-forwarding environment variable rename validations passed!"
exit 0
