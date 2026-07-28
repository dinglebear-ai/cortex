#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

status=0
patterns=(
  'syslog:read'
  'syslog:admin'
  '"name":"syslog"'
  '"name": "syslog"'
  '/path/to/syslog'
  'plugins/syslog'
  'syslog.cortex'
  'docs/syslog.subdomain.conf'
  'syslog.subdomain.conf'
  'mcporter list syslog'
  'x-syslog-action-metadata'
  'x-syslog-agent-guidance'
)
# A bare 'jmagar/cortex' covers every carrier of the legacy namespace at once:
# github.com/, raw.githubusercontent.com/, ghcr.io/, and the plain owner/name
# form used by CORTEX_RMCP_REPO and CORTEX_INSTALL_REPO. The two prefixed
# patterns are kept for message clarity when the bare one fires.
source_identity_patterns=(
  'jmagar/cortex'
  'github.com/jmagar/cortex'
  'raw.githubusercontent.com/jmagar/cortex'
)

tracked_current_files=()
tracked_source_identity_files=()
while IFS= read -r path; do
  # Directory symlinks such as .beads and .lavra point outside this repository.
  # Their targets are operational history, not current public product surfaces.
  if [ -L "$path" ]; then
    continue
  fi
  case "$path" in
    scripts/check-public-identity.sh)
      continue
      ;;
    .beads/*|docs/plans/*|docs/runbooks/*|docs/sessions/*|docs/superpowers/*|CHANGELOG.md)
      # Archival issue data and historical docs intentionally preserve old names.
      continue
      ;;
    *)
      tracked_source_identity_files+=("$path")
      ;;
  esac
  case "$path" in
    CLAUDE.md|README.md|server.json|mcpb/manifest.json|config/*|scripts/*|.github/*|.claude-plugin/*|plugins/*|docs/*)
      tracked_current_files+=("$path")
      ;;
  esac
done < <(git ls-files)

if [ "${#tracked_current_files[@]}" -eq 0 ] || [ "${#tracked_source_identity_files[@]}" -eq 0 ]; then
  echo "[public-identity] FAIL — no tracked current files selected for scan" >&2
  exit 1
fi

# The scan set is every tracked non-archival file, which includes binaries and
# LFS pointers. Force text mode in both implementations so the gate behaves the
# same with and without rg: bare `grep -F` reports "Binary file X matches" and
# exits 0 (a loud false FAIL), while bare `rg` skips binary content and exits 1
# (a silent miss).
search_name="grep"
search_status_error=2
search_current_files() {
  grep -naF -- "$1" "${tracked_current_files[@]}"
}
search_source_identity_files() {
  grep -naF -- "$1" "${tracked_source_identity_files[@]}"
}

if command -v rg >/dev/null 2>&1; then
  search_name="rg"
  search_current_files() {
    rg -n --text --fixed-strings -- "$1" "${tracked_current_files[@]}"
  }
  search_source_identity_files() {
    rg -n --text --fixed-strings -- "$1" "${tracked_source_identity_files[@]}"
  }
fi

for pattern in "${patterns[@]}"; do
  set +e
  search_current_files "$pattern"
  search_status=$?
  set -e
  if [ "$search_status" -eq 0 ]; then
    echo "[public-identity] FAIL — stale identity token found: $pattern" >&2
    status=1
  elif [ "$search_status" -eq "$search_status_error" ]; then
    echo "[public-identity] FAIL — $search_name failed while scanning for: $pattern" >&2
    status=1
  elif [ "$search_status" -ne 1 ]; then
    echo "[public-identity] FAIL — unexpected $search_name exit $search_status while scanning for: $pattern" >&2
    status=1
  fi
done

for pattern in "${source_identity_patterns[@]}"; do
  set +e
  search_source_identity_files "$pattern"
  search_status=$?
  set -e
  if [ "$search_status" -eq 0 ]; then
    echo "[public-identity] FAIL — stale source repository identity found: $pattern" >&2
    status=1
  elif [ "$search_status" -eq "$search_status_error" ]; then
    echo "[public-identity] FAIL — $search_name failed while scanning for: $pattern" >&2
    status=1
  elif [ "$search_status" -ne 1 ]; then
    echo "[public-identity] FAIL — unexpected $search_name exit $search_status while scanning for: $pattern" >&2
    status=1
  fi
done

if [ "$status" -eq 0 ]; then
  echo "[public-identity] OK — public docs/config use cortex identity"
fi

exit "$status"
