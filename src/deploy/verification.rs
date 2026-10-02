//! Read-only, authenticated verification inside the selected managed container.
use std::collections::BTreeMap;
use std::io;

use serde_json::Value;

use super::remote_assets::compose_command;
use super::remote_support::{RemoteRunner, remote_phase};
use crate::setup::{PhaseTimer, SetupPhase, SetupStatus};

pub(super) fn readiness_phase(
    runner: &mut dyn RemoteRunner,
    host: &str,
    home: &str,
    seconds: u64,
) -> io::Result<SetupPhase> {
    remote_phase(
        runner,
        host,
        "remote-health",
        &format!(
            "{}\ndeadline=$(( $(date +%s) + {} ))\nwhile [ \"$(date +%s)\" -lt \"$deadline\" ]; do\n  if compose exec -T cortex curl -fsS --connect-timeout 2 --max-time 5 http://127.0.0.1:3100/health >/dev/null; then echo 'ready after startup and migrations'; exit 0; fi\n  sleep 1\ndone\necho 'Readiness deadline exceeded; inspect cortex compose logs for migration/startup progress. Verified recovery snapshot is retained.' >&2\nexit 1",
            compose_command(home),
            seconds.clamp(1, 600),
        ),
        None,
    )
}

pub(super) fn verify_remote_server(
    runner: &mut dyn RemoteRunner,
    host: &str,
    home: &str,
    env: &BTreeMap<String, String>,
) -> io::Result<Vec<SetupPhase>> {
    let oauth_only = env
        .get("CORTEX_AUTH_MODE")
        .is_some_and(|value| value == "oauth")
        && env
            .get("CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH")
            .is_none_or(|value| value != "false");
    let script = format!(
        r#"{}
compose exec -T cortex sh -s <<'__CORTEX_VERIFY__'
set -eu
umask 077
config=$(mktemp)
trap 'rm -f "$config"' EXIT
# Credentials stay inside the container, in a private curl config; they are
# neither part of process arguments nor returned to the deployment client.
request() {{
  token=$1
  shift
  : > "$config"
  if test -n "$token"; then
    cr=$(printf '\r')
    case "$token" in *"$cr"*|*'
'*) echo 'credential must not contain a newline' >&2; exit 65;; esac
    escaped=$(printf '%s' "$token" | sed 's/\\/\\\\/g; s/"/\\"/g')
    printf 'header = "Authorization: Bearer %s"\n' "$escaped" > "$config"
  fi
  curl --config "$config" --fail --silent --show-error --connect-timeout 2 --max-time 10 "$@"
}}
if test "${{CORTEX_AUTH_MODE:-token}}" = oauth && test "${{CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH:-true}}" != false; then
  printf 'null\n__CORTEX_REPLY__\nnull\n__CORTEX_REPLY__\n'
else
  request "${{CORTEX_TOKEN:-}}" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-03-26","capabilities":{{}},"clientInfo":{{"name":"cortex-deploy","version":"1"}}}}}}' http://127.0.0.1:3100/mcp
  printf '\n__CORTEX_REPLY__\n'
  request "${{CORTEX_TOKEN:-}}" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"cortex","arguments":{{"action":"status"}}}}}}' http://127.0.0.1:3100/mcp
  printf '\n__CORTEX_REPLY__\n'
fi
test -n "${{CORTEX_API_TOKEN:-}}" || {{ echo 'REST API credential missing' >&2; exit 65; }}
request "$CORTEX_API_TOKEN" http://127.0.0.1:3100/api/stats
printf '\n__CORTEX_REPLY__\n'
request "$CORTEX_API_TOKEN" http://127.0.0.1:3100/api/ingest-rate
__CORTEX_VERIFY__
"#,
        compose_command(home)
    );
    let timer = PhaseTimer::start("remote-auth-verification");
    let output = runner.run(host, &script, None)?;
    if !output.status_success {
        // The script's stdout contains responses. Do not include them in errors.
        return Ok(vec![timer.finish(SetupStatus::Error, "authenticated server verification failed; check MCP/REST policy and selected credentials. Recovery snapshot is retained")]);
    }
    match validate_replies(&output.stdout, oauth_only) {
        Ok(()) => {
            let mut phases = vec![timer.finish(SetupStatus::Ok, "authenticated REST stats and ingestion status verified; delivery still depends on configured sources")];
            if oauth_only {
                phases.push(PhaseTimer::start("remote-mcp-auth").finish(SetupStatus::Warn, "needs_configuration: OAuth-only MCP requires a fresh client login; REST was verified without enabling static tokens"));
            }
            Ok(phases)
        }
        Err(message) => Ok(vec![timer.finish(SetupStatus::Error, message)]),
    }
}

fn parse_reply(raw: &str) -> Result<Value, &'static str> {
    serde_json::from_str(raw.trim())
        .or_else(|_| {
            raw.lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .find_map(|line| serde_json::from_str(line.trim()).ok())
                .ok_or_else(|| serde_json::from_str::<Value>("").unwrap_err())
        })
        .map_err(|_| "server verification returned an invalid JSON response")
}

fn validate_replies(raw: &str, oauth_only: bool) -> Result<(), &'static str> {
    let replies = raw
        .split("__CORTEX_REPLY__")
        .map(parse_reply)
        .collect::<Result<Vec<_>, _>>()?;
    if replies.len() != 4 {
        return Err("server verification returned incomplete replies");
    }
    if !oauth_only {
        if replies[0].get("id") != Some(&Value::from(1))
            || replies[0].get("error").is_some()
            || replies[0].pointer("/result/serverInfo").is_none()
        {
            return Err("MCP initialization failed or returned no server identity");
        }
        if replies[1].get("id") != Some(&Value::from(2))
            || replies[1].get("error").is_some()
            || replies[1]
                .pointer("/result/isError")
                .and_then(Value::as_bool)
                == Some(true)
            || replies[1].pointer("/result/content").is_none()
        {
            return Err("authenticated Cortex status action failed");
        }
    }
    if !replies[2].is_object() || !replies[3].is_object() {
        return Err("authenticated REST verification returned a non-object response");
    }
    Ok(())
}

#[cfg(test)]
#[path = "verification_tests.rs"]
mod tests;
