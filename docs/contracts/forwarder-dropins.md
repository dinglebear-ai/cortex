---
title: "Forwarder Drop-in Templates & Agent Enrollment (V1)"
created: 2026-05-16
updated: 2026-07-30
---

# Forwarder Drop-in Templates & Agent Enrollment (V1)

## 1. Purpose & status

This document is the canonical operator runbook for **getting logs into
`cortex`**. It is normative for every onboarded host: every host MUST be
reachable via exactly one of the templates in §6, and the agent enrollment
flow in §8 is the authoritative UX for the WebSocket agent (Epic A,
`cortex-qgnx`).

The contract is **V1**. Changes to wire protocol behavior (RFC 3164/5424/CEF
handling), templates, or the agent enrollment commands must update this file
in the same PR as the code change.

Cross-references:

- HTTP endpoint stability, auth, and rate-limit policy:
  [`http-endpoints.md`](http-endpoints.md). The agent enrollment flow uses
  `WS /ws/agent` from that catalog.
- JSON-RPC wire format for the agent channel, including methods, error
  codes, and state machine: [`agent-protocol.md`](agent-protocol.md).
- Existing deploy artifacts referenced below live under
  `deploy/rsyslog/`, `deploy/otel/`, and `deploy/apparmor/` at repo root.

## 2. Forwarding paths overview

Pick the **first** option that fits the host:

| Host type                                                 | Recommended path                                                                                                            |
| --------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| Linux host where you can install a binary (`<CORTEX_HOST>`'s peers) | **Direct WSS agent** (§8). Gets you bidirectional control, probes, metrics, durable local buffer.                            |
| Linux host with rsyslog already present and no agent      | **rsyslog → `:1514`** (Template A or B in §6).                                                                              |
| Linux host with syslog-ng instead of rsyslog              | **syslog-ng → `:1514`** (Template D).                                                                                       |
| WSL2 host (<WINDOWS_WSL_HOST>, <LAPTOP_WSL_HOST>)                      | **rsyslog → Tailscale IP** (Template E). Requires `[boot] systemd=true` in `/etc/wsl.conf`.                                  |
| AI transcript hosts (<DEV_HOST>, <EDGE_HOST>, <WINDOWS_WSL_HOST>, <LAPTOP_WSL_HOST>) | rsyslog `imfile` drop-in tailing `~/.claude/projects/*/*.jsonl`, etc. (`deploy/rsyslog/40-ai-transcripts.conf`). |
| OTel-instrumented apps (Claude Code, Codex)               | **OTLP → `POST /v1/logs`** with Bearer token. See `deploy/otel/`.                                                            |
| Network gear (UniFi, switches, printers, routers)         | **UDP RFC 3164 → `:1514`** only. No agent option. Source IP gating recommended.                                              |
| Stateful APIs with no log stream (UniFi controller, AdGuard Home) | **server-side poller** (Epic `cortex-awvr`). No host config — see `docs/superpowers/specs/2026-05-16-api-pollers-design.md`. |

Pollers are **outbound** from `cortex` and require no host-side drop-in.
They are listed for completeness; nothing in this file applies to them.

## 3. Minimum versions

| Tool       | Minimum    | Reason                                                                                                                                  |
| ---------- | ---------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| rsyslog    | **8.x**    | `imjournal` reliability fixes, `imfile` `addMetadata` support, and the `module(load="imfile")` once-per-process rule used by `11-imfile.conf`. rsyslog 8.2504 specifically rejects duplicate `imfile` loads. |
| syslog-ng  | **3.27**   | First version with stable `syslog()` destination + `flags(syslog-protocol)` for RFC 5424.                                               |
| journald   | systemd ≥ 230 | Required for `imjournal` `StateFile` persistence.                                                                                        |
| WSL2       | with `[boot] systemd=true` in `/etc/wsl.conf` | Without systemd, rsyslog can't tail journald and the AI transcript drop-in won't have a working `systemctl restart`. |
| syslog agent binary | matches server `min_agent_version` (server-config gated; agent learns the floor from `-32003 AgentVersionUnsupported.data`) | Enforces the protocol version handshake in `agent-protocol.md` §3. |

## 4. Wire protocol behavior

What the listener accepts on `:1514` (UDP and TCP):

| Format                                  | Accepted | Notes                                                                                                                                                |
| --------------------------------------- | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| **RFC 3164** (legacy BSD syslog)        | yes      | No millisecond precision. Network gear and old rsyslog defaults emit this. Severity/facility decoded from the `<pri>` byte.                          |
| **RFC 5424** (modern, with structured data) | yes  | Preferred. `STRUCTURED-DATA` slot is parsed into the row's `metadata` JSON. Use this from rsyslog with `template(name="..." type="list" option.stdsql="off")` + `RSYSLOG_SyslogProtocol23Format`. |
| **CEF (Common Event Format)**           | partial  | UniFi gear emits CEF. The CEF header (`CEF:0\|vendor\|product\|...`) is parsed where we can; the extension key/value pairs land in `message`. Treat as best-effort until the UniFi poller (Epic `cortex-awvr`) lands. |
| **Plaintext / non-RFC**                 | yes      | Stored raw. Severity defaults to `info` (facility `user`). `app_name` is extracted heuristically from a leading `tag:` if present.                  |
| **TLS-wrapped syslog** (RFC 5425)       | **no**   | V1 syslog listener is TCP/UDP only. Tracked as deferred future work. Use the WSS agent (§8) when transport security matters.                          |

Single-message size cap on the listener side matches rsyslog's `$MaxMessageSize` default of 8 KiB unless you raise it on the sender (see `deploy/rsyslog/11-imfile.conf`, which sets 256 KiB host-side).

## 5. Self-loop avoidance

Every rsyslog drop-in that forwards `*.*` MUST start with this filter:

```rsyslog
if ($programname == "syslog" or $programname == "rsyslogd") then stop
```

Without it, the rsyslog daemon's own logs are forwarded back to cortex,
which re-emits them, which the daemon then forwards — a feedback loop that
can saturate the listener inside a minute. Always include this header when
writing or pushing a drop-in by hand.

## 6. Canonical templates

For each template, `<SERVER>` is the cortex host (`<CORTEX_HOST>` LAN IP, the
Tailscale IP `<CORTEX_SERVER_IP>`, or `syslog.example.invalid` if DNS resolves on the
sender). `<PORT>` is `mcp.syslog_port` (default `1514`).

### Template A — rsyslog TCP forward (preferred)

```rsyslog
# /etc/rsyslog.d/99-cortex.conf
# Avoid feeding cortex/rsyslog internal logs back into cortex.
if ($programname == "syslog" or $programname == "rsyslogd") then stop
*.* @@<SERVER>:<PORT>
```

`@@` selects TCP. Reliable delivery (the kernel will retransmit; rsyslog
queues on disconnect via `ActionQueueType=LinkedList` defaults). Use this
for every host that can reach the server over TCP.

### Template B — rsyslog UDP forward (fallback)

```rsyslog
# /etc/rsyslog.d/99-cortex.conf
if ($programname == "syslog" or $programname == "rsyslogd") then stop
*.* @<SERVER>:<PORT>
```

`@` selects UDP. Fire-and-forget; packet loss is silent. Use only when TCP
isn't reachable (some embedded gear). Loss characteristics: any UDP frame
that exceeds the host's `net.core.wmem_max` or the network path's MTU is
dropped without notice; rsyslog has no retransmit for `@`.

### Template C — rsyslog with rate limiting

For high-volume hosts where a runaway service could drown the channel.
Cap the sender, not the receiver — server-side enforcement is via the storage
guardrail (`max_db_size_mb`), which is too coarse for protecting other
hosts' liveness.

```rsyslog
# /etc/rsyslog.d/99-cortex.conf
if ($programname == "syslog" or $programname == "rsyslogd") then stop

# Cap forwards at 5000 messages / second, burst 20000.
# Tune both knobs to fit the host's normal baseline + 5x headroom.
$SystemLogRateLimitInterval 1
$SystemLogRateLimitBurst 20000

action(type="omfwd"
       target="<SERVER>" port="<PORT>" protocol="tcp"
       queue.type="LinkedList"
       queue.size="50000"
       queue.dequeuebatchsize="1000"
       queue.spoolDirectory="/var/spool/rsyslog"
       queue.filename="cortex_queue"
       queue.saveonshutdown="on"
       action.resumeRetryCount="-1")
```

`action.resumeRetryCount="-1"` means rsyslog keeps retrying indefinitely
across server outages; the disk-backed `LinkedList` queue gives you a hard
upper bound of 50000 messages in transit.

### Template D — syslog-ng equivalent of Template A

```syslog-ng
# /etc/syslog-ng/conf.d/99-cortex.conf
destination d_cortex {
  syslog(
    "<SERVER>"
    transport("tcp")
    port(<PORT>)
    flags(syslog-protocol)
    so_keepalive(yes)
  );
};

filter f_not_self {
  not (program("syslog") or program("syslog-ng"));
};

log { source(s_src); filter(f_not_self); destination(d_cortex); };
```

`flags(syslog-protocol)` forces RFC 5424 framing — preferred over the
syslog-ng legacy format. `s_src` is whatever the host already defines as the
journald+kernel source (usually `s_src`, sometimes `s_local`).

### Template E — WSL-specific (Tailscale routing)

WSL2 has its own network namespace and **cannot** reach the Windows host's
loopback — `127.0.0.1:1514` on WSL is not the cortex host. Two viable
options:

1. **Tailscale (recommended).** Install Tailscale inside WSL, join the
   tailnet, point at the server's Tailscale IP. Example for <CORTEX_HOST>:

   ```rsyslog
   # /etc/rsyslog.d/99-cortex.conf
   if ($programname == "syslog" or $programname == "rsyslogd") then stop
   *.* @@<CORTEX_SERVER_IP>:1514
   ```

2. **WSL eth0 → Windows host IP.** Use the host's LAN IP (which WSL CAN
   reach via the default route) and forward through Windows. Brittle when
   the Windows host's DHCP lease changes; prefer Tailscale.

Prerequisites for either:

- `/etc/wsl.conf` contains `[boot]\nsystemd=true`, then `wsl --shutdown`
  from PowerShell to restart. After restart, `systemctl status` must show
  "running".
- `/var/spool/rsyslog` exists with `syslog:syslog` ownership.

## 7. AppArmor profile

On Ubuntu/Debian hosts with AppArmor in enforcing mode, the default
`/etc/apparmor.d/usr.sbin.rsyslogd` profile permits `/var/log/**` but not
the homelab service paths under `/mnt/appdata/**` or user transcript paths
under `~/.claude`, `~/.codex`, `~/.gemini`. The local override at
`deploy/apparmor/usr.sbin.rsyslogd.cortex` extends the profile with
exactly the paths used by the file-tail drop-ins on <EDGE_HOST>.

Install:

```bash
sudo install -o root -g root -m 0644 \
  deploy/apparmor/usr.sbin.rsyslogd.cortex \
  /etc/apparmor.d/local/usr.sbin.rsyslogd
sudo apparmor_parser -r /etc/apparmor.d/usr.sbin.rsyslogd
```

The profile is only needed on hosts that run file-tail drop-ins outside
`/var/log/**`. Pure network-forwarder hosts don't need it. The file in the
repo is <EDGE_HOST>-specific; adapt the path list if you're adding a new
file-tail source on another host.

## 8. Agent enrollment flow (Epic A)

UX for installing the WSS agent on a new host. The agent talks
`wss://syslog.example.invalid/ws/agent` and authenticates with a per-host
long-lived token (BLAKE3-hashed server-side). See
[`agent-protocol.md`](agent-protocol.md) §3 for the wire-level handshake.

### Step 1 — Server (on `<CORTEX_HOST>`): issue a one-time enrollment token

```bash
# Single host
cortex agent issue --host=<DEV_HOST>

# Output (token is shown ONCE; re-run if lost):
#   host_id:       2b9a0b3a-7e3c-4d2a-9c0e-9bbf5d3a1f01
#   enrollment_token: c2VjcmV0LXRva2VuLW9wYXF1ZS1iYXNlNjR1cmwtMzJieXRlcw
#   expires_at:    2026-05-16T20:00:00Z   (15 min default)
```

Token is one-time. The DB row in `agents` has the **hashed** token; the
plaintext only exists in the operator's clipboard.

### Step 2 — Target host: install the agent binary

The agent ships as the same `cortex` binary that runs on the server, but
invoked in agent mode (`cortex agent ...`). Install via the homelab's
preferred mechanism (cargo install, deb, or direct binary drop into
`/usr/local/bin/syslog`).

### Step 3 — Target host: enroll with the one-time token

```bash
sudo syslog agent enroll <enrollment_token> --server=wss://syslog.example.invalid/ws/agent
```

The enroll subcommand:
1. Connects to `/ws/agent` with subprotocol `cortex.v1`.
2. Sends `agent.hello` (see `agent-protocol.md` §4.1) with the
   enrollment token.
3. On success, the server rotates the token: the enrollment token is
   invalidated and a long-lived credential is issued in the
   `HelloResult` payload (or the server immediately follows with a
   `config.update` carrying the rotated token — exact mechanism is
   pinned in epic D).
4. The long-lived token is written to **`/var/lib/syslog-agent/token`**
   with mode `0600`, owner `syslog-agent:syslog-agent`. This path is
   normative — see `docs/contracts/data-layout.md`.

### Step 4 — Target host: install the systemd unit and start

Drop the following at `/etc/systemd/system/syslog-agent.service`:

```ini
[Unit]
Description=cortex agent
Documentation=https://github.com/dinglebear-ai/cortex/blob/main/docs/contracts/forwarder-dropins.md
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
ExecStart=/usr/local/bin/cortex agent run
Restart=on-failure
RestartSec=5s
User=syslog-agent
Group=syslog-agent

# Local buffer + token path.
StateDirectory=syslog-agent
StateDirectoryMode=0700

# Hardening — agent doesn't need most of the kitchen.
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
NoNewPrivileges=yes
ReadWritePaths=/var/lib/syslog-agent

# Let it read journald, mostly.
SupplementaryGroups=systemd-journal

[Install]
WantedBy=multi-user.target
```

Then:

```bash
sudo useradd --system --home-dir /var/lib/syslog-agent --shell /usr/sbin/nologin syslog-agent
sudo systemctl daemon-reload
sudo systemctl enable --now syslog-agent
```

### Step 5 — Server: verify the new host is active

```bash
cortex agent list

# Expected:
#   host_id                                hostname        state    last_seen
#   2b9a0b3a-7e3c-4d2a-9c0e-9bbf5d3a1f01   <DEV_HOST>          Active   2026-05-16T19:43:01Z
```

The `Active` state is set when `agent.hello` succeeds and the WebSocket
transitions per the state machine in `agent-protocol.md` §6.

## 9. Verification (any forwarding path)

After installing any template, run this two-command sanity check from the
target host and the server respectively:

**On the host:**

```bash
logger -t deploy-test "hello from $(hostname)"
```

**On the server (within ~5 seconds):**

```bash
# replace <target-host> with the actual hostname
cortex tail --host=<target-host> --limit=5
# OR via MCP:
mcporter call --config config/mcporter.json cortex.search query=deploy-test limit=5
```

For agent-mode hosts, also check:

```bash
cortex agent list                      # Active state, recent last_seen
cortex tail --host=<host> --limit=5  # logs.push entries landing
```

For OTLP hosts:

```bash
curl -s http://<server>:3100/health | jq '.otlp_logs_received'
# Should increment after any OTel exporter flush.
```

## 10. Decommissioning

### Rsyslog-only host

```bash
ssh <host> '
  sudo rm -f /etc/rsyslog.d/99-cortex.conf \
             /etc/rsyslog.d/40-ai-transcripts.conf  # if present
  sudo rsyslogd -N1
  sudo systemctl restart rsyslog
'
```

No server-side cleanup required — the host's rows in `logs` will age out via
retention.

### Agent host

```bash
# Server-side: revoke first so the agent can't reconnect.
cortex agent revoke <host_id>

# Host-side: stop the service and remove the token.
ssh <host> '
  sudo systemctl disable --now syslog-agent
  sudo rm -f /var/lib/syslog-agent/token
  sudo rm -f /etc/systemd/system/syslog-agent.service
  sudo systemctl daemon-reload
'
```

`cortex agent revoke` sets `agents.connection_state = 'Revoked'` and zeros
the `token_hash`. The next `agent.hello` from that host receives
`-32002 TokenRevoked` and WS close `4002`; the agent's `enroll` step left
behind the token at `/var/lib/syslog-agent/token`, and per
`agent-protocol.md` §3 the agent deletes it on receiving `-32002` and
transitions to terminal `Revoked` state. The manual `rm -f` above is
belt-and-braces for the case where the agent was stopped before it got the
revoke message.

## 11. Per-host inventory (current)

Tracked in
`~/.claude/projects/-home-jmagar-workspace-cortex/memory/MEMORY.md`
under "Infrastructure & Deployment". Snapshot at time of writing:

| Host          | Forwarding path                                                                                          |
| ------------- | -------------------------------------------------------------------------------------------------------- |
| `<CORTEX_HOST>`      | **Server itself.** Hosts the listener, MCP server, and (planned) agent endpoint. No drop-in.              |
| `<DEV_HOST>`      | rsyslog TCP → `:1514` (Template A) + `deploy/rsyslog/10-imjournal.conf` + AI transcripts drop-in.        |
| `<EDGE_HOST>`     | rsyslog TCP → `:1514` (Template A) + imjournal + AppArmor override + SWAG, Authelia, AdGuard file tails. |
| `<WINDOWS_WSL_HOST>`  | rsyslog TCP → `<CORTEX_SERVER_IP>:1514` (Template E, Tailscale) + imjournal + AI transcripts.                  |
| `<LAPTOP_WSL_HOST>`| rsyslog TCP → `<CORTEX_SERVER_IP>:1514` (Template E, Tailscale) + imjournal + AI transcripts.                  |
| `<BACKUP_HOST>`       | rsyslog TCP → `:1514` (Template A).                                                                       |

When the agent rollout (Epic A) ships, migrate each Linux host above from
the rsyslog drop-in to the WSS agent. The rsyslog drop-in stays in place for
fallback during the cutover, then is removed via §10.

## 12. Troubleshooting matrix

| Symptom                                  | Check                                                                                                    |
| ---------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| No logs from `<host>`                    | (a) `ssh <host> systemctl status rsyslog` — is the daemon running?                                       |
|                                          | (b) `ssh <host> sudo rsyslogd -N1` — does the config parse?                                              |
|                                          | (c) on the server: `sudo tcpdump -ni any port 1514` — are frames arriving?                              |
|                                          | (d) `cortex hosts` — does `<host>` appear in the known-hosts list at all?                                |
|                                          | (e) `cortex silent_hosts` — is it a known host that's gone quiet?                                       |
| Loop / message storm                     | The self-loop filter from §5 is missing or out of order. Inspect `/etc/rsyslog.d/99-cortex.conf`.   |
| WSL host can't reach `<CORTEX_HOST>`            | Confirm Tailscale up: `tailscale status` inside WSL. Use Template E with the Tailscale IP.               |
| AppArmor blocks file tail on <EDGE_HOST>     | `sudo aa-status` then `sudo dmesg | grep DENIED` — install the override from §7 and reload.              |
| OTLP `/v1/logs` returns 401              | `Authorization: Bearer <CORTEX_TOKEN>` not set or wrong. Check `curl -s http://<server>:3100/health` works without auth, then add the header. |
| OTLP `/v1/logs` returns 413              | Payload exceeded 4 MiB. Reduce OTel exporter batch size (`OTEL_EXPORTER_OTLP_LOGS_TIMEOUT` / batch size). Note the `Retry-After: 86400` — exporters will back off for a day. |
| OTLP `/v1/logs` returns 503              | Server ingest channel saturated. Either the listener is offline (check `cortex db status`) or the writer task crashed (check `cortex` container logs). |
| Agent connects then immediately drops    | Check the WS close code in the agent's logs. `4001` = auth failed, `4002` = revoked, `4000` = handshake timeout, `1009` = oversized frame, `1011` = missed pongs. Each maps to a remediation in `agent-protocol.md` §5. |
| Agent in `Reconnecting` forever          | Server unreachable, or `protocol_version`/`agent_version` mismatch. Look for `-32003 AgentVersionUnsupported` in the agent log — it carries `data.required_protocol_version`. |

## 13. Self-check

- Every host in §11 maps to exactly one template in §6 (or is the server
  itself).
- The agent enrollment flow in §8 is end-to-end: issue → install → enroll →
  systemd unit → verify.
- The self-loop filter in §5 is reproduced verbatim in every rsyslog
  template that forwards `*.*`.
- Every wire format in §4 either has explicit support or an explicit "no"
  with a reason.
- Decommissioning in §10 covers both the rsyslog-only and agent paths.
