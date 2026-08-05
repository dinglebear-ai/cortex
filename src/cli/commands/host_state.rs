//! Parse function for `cortex state host`.
//!
//! Surface parity (cxih.4): exposes the `host_state` MCP action and
//! `GET /api/host-state` REST route through the nested CLI command.

use anyhow::{Result, bail};

use super::super::argdefaults::positional_value;
use super::super::args::HostStateArgs;
use super::super::{FlagCursor, norm_time, parse_u32_flag};

pub(crate) fn parse_host_state_args(args: &[String]) -> Result<HostStateArgs> {
    let mut parsed = HostStateArgs::default();
    let mut positionals: Vec<String> = Vec::new();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--host-id")? {
            parsed.host_id = Some(v);
        } else if let Some(v) = flags.match_value(&arg, "--host")? {
            parsed.host = Some(v);
        } else if let Some(v) = flags.match_value(&arg, "--since")? {
            parsed.since = Some(norm_time(v)?);
        } else if let Some(v) = flags.match_value(&arg, "--limit")? {
            parsed.limit = Some(parse_u32_flag("--limit", v)?);
        } else if arg.starts_with('-') {
            bail!(
                "{}",
                super::super::suggest::unknown_option(
                    "state host",
                    &arg,
                    &["--json", "--host-id", "--host", "--since", "--limit"],
                )
            );
        } else {
            // A bare positional binds to --host (e.g. `cortex state host devhost`).
            positionals.push(arg);
        }
    }
    if let Some(host) = positional_value("host_state", &positionals)? {
        if parsed.host.is_some() {
            bail!("--host and a positional host are mutually exclusive");
        }
        parsed.host = Some(host);
    }
    Ok(parsed)
}
