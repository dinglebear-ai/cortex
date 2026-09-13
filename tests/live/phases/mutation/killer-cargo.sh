#!/usr/bin/env bash
set -euo pipefail
# Select exactly the manifest's behavioral test; driver verifies its terminal
# libtest result rather than treating an arbitrary Cargo exit as a kill.
: "${MUTANT_TEST:?mapped behavioral test required}"
exec cargo test --lib --locked "$MUTANT_TEST" -- --exact --format pretty --test-threads=1
