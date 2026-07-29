//! Linux kernel parser — OOM kills, link state, MAC collisions.
//! Spec: docs/superpowers/specs/2026-05-16-enrichment-framework-design.md §7.1

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, json};

use crate::enrich::{Parser, ParserError, ParserInput, ParserOutput};

pub struct KernelParser;

static OOM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^Out of memory: Killed process (?P<pid>\d+) \((?P<comm>[^)]+)\) total-vm:(?P<vm>\d+)kB, anon-rss:(?P<rss>\d+)kB.* UID:(?P<uid>\d+).*oom_score_adj:(?P<adj>-?\d+)",
    )
    .expect("static regex")
});

static LINK_UP_RE: LazyLock<Regex> = LazyLock::new(|| {
    // [\w.\-@]+ covers real Linux interface names: br-abc.100, veth-foo, eth0.100, wlan0@phy0
    Regex::new(r"^(?P<if>[\w.\-@]+): link up,\s*(?P<speed>\d+)Mbps").expect("static regex")
});

static LINK_DOWN_RE: LazyLock<Regex> = LazyLock::new(|| {
    // [\w.\-@]+ covers real Linux interface names: br-abc.100, veth-foo, eth0.100, wlan0@phy0
    Regex::new(r"^(?P<if>[\w.\-@]+): link down").expect("static regex")
});

static MAC_COLLISION_RE: LazyLock<Regex> = LazyLock::new(|| {
    // [\w.\-@]+ covers real Linux interface names: br-abc.100, veth-foo, eth0.100, wlan0@phy0
    Regex::new(
        r"^(?P<if>[\w.\-@]+): received packet on \S+ with own address as source address \(addr:(?P<mac>[0-9a-f:]+)(?:, vlan:(?P<vlan>\d+))?\)",
    )
    .expect("static regex")
});

impl Parser for KernelParser {
    fn name(&self) -> &'static str {
        "kernel"
    }

    fn namespace(&self) -> &'static str {
        "kernel"
    }

    fn parse(&self, input: ParserInput<'_>) -> Result<ParserOutput, ParserError> {
        let msg = input.message;
        if msg.starts_with("Out of memory:") {
            return parse_oom(msg);
        }
        if msg.contains(": link up") || msg.contains(": link down") {
            return parse_link(msg);
        }
        if msg.contains("with own address as source address") {
            return parse_mac_collision(msg);
        }
        Err(ParserError::NoMatch("not a recognised kernel pattern"))
    }
}

fn parse_oom(msg: &str) -> Result<ParserOutput, ParserError> {
    let caps = OOM_RE
        .captures(msg)
        .ok_or(ParserError::NoMatch("oom_killer line malformed"))?;
    let mut metadata = Map::new();
    metadata.insert("pid".into(), json!(caps["pid"].parse::<i64>().unwrap_or(0)));
    metadata.insert("comm".into(), json!(&caps["comm"]));
    metadata.insert(
        "total_vm_kb".into(),
        json!(caps["vm"].parse::<i64>().unwrap_or(0)),
    );
    metadata.insert(
        "anon_rss_kb".into(),
        json!(caps["rss"].parse::<i64>().unwrap_or(0)),
    );
    metadata.insert("uid".into(), json!(caps["uid"].parse::<i32>().unwrap_or(0)));
    metadata.insert(
        "oom_score_adj".into(),
        json!(caps["adj"].parse::<i32>().unwrap_or(0)),
    );
    Ok(ParserOutput {
        event_action: Some("oom_kill".into()),
        severity: Some("crit"),
        metadata,
        ..Default::default()
    })
}

fn parse_link(msg: &str) -> Result<ParserOutput, ParserError> {
    let mut metadata = Map::new();
    if let Some(caps) = LINK_UP_RE.captures(msg) {
        metadata.insert("interface".into(), json!(&caps["if"]));
        metadata.insert("state".into(), json!("up"));
        if let Ok(speed) = caps["speed"].parse::<i32>() {
            metadata.insert("speed_mbps".into(), json!(speed));
        }
        return Ok(ParserOutput {
            event_action: Some("link_up".into()),
            metadata,
            ..Default::default()
        });
    }
    if let Some(caps) = LINK_DOWN_RE.captures(msg) {
        metadata.insert("interface".into(), json!(&caps["if"]));
        metadata.insert("state".into(), json!("down"));
        return Ok(ParserOutput {
            event_action: Some("link_down".into()),
            metadata,
            ..Default::default()
        });
    }
    Err(ParserError::NoMatch("link line malformed"))
}

fn parse_mac_collision(msg: &str) -> Result<ParserOutput, ParserError> {
    let caps = MAC_COLLISION_RE
        .captures(msg)
        .ok_or(ParserError::NoMatch("mac collision malformed"))?;
    let mut metadata = Map::new();
    metadata.insert("interface".into(), json!(&caps["if"]));
    metadata.insert("colliding_mac".into(), json!(&caps["mac"]));
    if let Some(vlan) = caps.name("vlan")
        && let Ok(v) = vlan.as_str().parse::<i32>()
    {
        metadata.insert("vlan".into(), json!(v));
    }
    Ok(ParserOutput {
        event_action: Some("mac_collision".into()),
        metadata,
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "kernel_tests.rs"]
mod kernel_tests;
