(() => {
  "use strict";

  const appVersion = "investigation-v1";
  let bearerToken = "";
  let cy = null;
  let latestHosts = [];
  let latestLogs = [];

  const $ = (selector) => document.querySelector(selector);

  const ui = {
    tokenForm: $("[data-token-form]"),
    askForm: $("[data-ask-form]"),
    answerStack: $("[data-answer-stack]"),
    serverVersion: $("[data-server-version]"),
    schemaVersion: $("[data-schema-version]"),
    hostCount: $("[data-host-count]"),
    logCount: $("[data-log-count]"),
    v1State: $("[data-v1-state]"),
    selectedTitle: $("[data-selected-title]"),
    selectedKind: $("[data-selected-kind]"),
    evidenceList: $("[data-evidence-list]"),
    timeline: $("[data-timeline]"),
    logStatus: $("[data-log-status]"),
    fitGraph: $("[data-fit-graph]"),
    refresh: $("[data-refresh]"),
    clearToken: $("[data-clear-token]"),
  };

  const seed = {
    nodes: [
      { data: { id: "cortex", label: "cortex", kind: "service", status: "degraded" } },
      { data: { id: "devhost", label: "devhost", kind: "host", status: "unknown" } },
      { data: { id: "edgehost", label: "edgehost", kind: "host", status: "unknown" } },
      { data: { id: "nashost", label: "nashost", kind: "host", status: "unknown" } },
      { data: { id: "sqlite", label: "SQLite WAL", kind: "store", status: "unknown" } },
    ],
    edges: [
      { data: { id: "devhost-cortex", source: "devhost", target: "cortex", label: "serves" } },
      { data: { id: "edgehost-cortex", source: "edgehost", target: "cortex", label: "forwards" } },
      { data: { id: "nashost-cortex", source: "nashost", target: "cortex", label: "forwards" } },
      { data: { id: "cortex-sqlite", source: "cortex", target: "sqlite", label: "stores" } },
    ],
  };

  function themeValue(name) {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  }

  function graphTheme() {
    return {
      page: themeValue("--aurora-page-bg"),
      border: themeValue("--aurora-border-strong"),
      text: themeValue("--aurora-text-primary"),
      muted: themeValue("--aurora-text-muted"),
      accent: themeValue("--aurora-accent-primary"),
      accentStrong: themeValue("--aurora-accent-strong"),
      rose: themeValue("--aurora-accent-pink"),
      violet: themeValue("--aurora-accent-violet"),
      warn: themeValue("--aurora-warn"),
    };
  }

  function text(tag, className, value) {
    const el = document.createElement(tag);
    if (className) el.className = className;
    el.textContent = value == null ? "" : String(value);
    return el;
  }

  function clear(node) {
    while (node.firstChild) node.removeChild(node.firstChild);
  }

  function setBadge(node, label, tone = "neutral") {
    node.className = `badge ${tone}`;
    node.textContent = label;
  }

  function setV1State(title, detail, tone = "warn") {
    clear(ui.v1State);
    ui.v1State.className = `stat ${tone}`;
    ui.v1State.append(text("span", "meta-label", "Investigation API"));
    ui.v1State.append(text("strong", null, title));
    ui.v1State.append(text("small", null, detail));
  }

  function authHeaders() {
    return bearerToken ? { Authorization: `Bearer ${bearerToken}` } : {};
  }

  async function apiGet(path) {
    const response = await fetch(path, {
      headers: authHeaders(),
      cache: "no-store",
    });
    if (!response.ok) {
      const message = await response.text().catch(() => "");
      throw new Error(`${response.status} ${response.statusText} ${message}`.trim());
    }
    return response.json();
  }

  async function apiPost(path, body) {
    const response = await fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...authHeaders() },
      body: JSON.stringify(body),
      cache: "no-store",
    });
    if (!response.ok) {
      const message = await response.text().catch(() => "");
      throw new Error(`${response.status} ${response.statusText} ${message}`.trim());
    }
    return response.json();
  }

  function initGraph() {
    if (!window.cytoscape) {
      ui.evidenceList.replaceChildren(text("p", "muted", "Graph library failed to load."));
      return;
    }

    const theme = graphTheme();

    cy = window.cytoscape({
      container: document.getElementById("graph-canvas"),
      elements: seed,
      boxSelectionEnabled: true,
      style: [
        {
          selector: "node",
          style: {
            "background-color": theme.accent,
            "border-color": theme.accentStrong,
            "border-width": 2,
            "color": theme.text,
            "font-family": "Inter, sans-serif",
            "font-size": 12,
            "font-weight": 600,
            "label": "data(label)",
            "text-outline-color": theme.page,
            "text-outline-width": 3,
            "text-valign": "bottom",
            "text-margin-y": 8,
            "width": 42,
            "height": 42,
          },
        },
        {
          selector: 'node[kind = "host"]',
          style: { "background-color": theme.rose, "border-color": theme.rose },
        },
        {
          selector: 'node[kind = "store"]',
          style: { "shape": "round-rectangle", "background-color": theme.violet, "border-color": theme.violet },
        },
        {
          selector: 'node[status = "degraded"]',
          style: { "border-color": theme.warn, "border-width": 4 },
        },
        {
          selector: "edge",
          style: {
            "curve-style": "bezier",
            "line-color": theme.border,
            "target-arrow-color": theme.border,
            "target-arrow-shape": "triangle",
            "width": 2,
            "label": "data(label)",
            "font-family": "JetBrains Mono, monospace",
            "font-size": 9,
            "color": theme.muted,
            "text-background-color": theme.page,
            "text-background-opacity": .85,
            "text-background-padding": 2,
          },
        },
        {
          selector: ":selected",
          style: {
            "border-color": theme.accentStrong,
            "border-width": 5,
            "line-color": theme.accentStrong,
            "target-arrow-color": theme.accentStrong,
          },
        },
      ],
      layout: { name: "cose", animate: false, padding: 38 },
    });

    cy.on("tap", "node", (event) => showNodeEvidence(event.target.data()));
  }

  function graphFromHosts(hosts) {
    const uniqueHosts = Array.from(new Set(hosts.map(String))).slice(0, 36);
    const nodes = [
      { data: { id: "cortex", label: "cortex", kind: "service", status: "online" } },
      ...uniqueHosts.map((host) => ({
        data: { id: `host:${host}`, label: host, kind: "host", status: "online" },
      })),
      { data: { id: "sqlite", label: "SQLite WAL", kind: "store", status: "online" } },
    ];
    const edges = uniqueHosts.map((host) => ({
      data: {
        id: `host:${host}->cortex`,
        source: `host:${host}`,
        target: "cortex",
        label: "ingests",
      },
    }));
    edges.push({ data: { id: "cortex->sqlite", source: "cortex", target: "sqlite", label: "writes" } });
    return { nodes, edges };
  }

  function updateGraph(elements) {
    if (!cy) return;
    cy.elements().remove();
    cy.add(elements);
    cy.layout({ name: "cose", animate: false, padding: 38 }).run();
  }

  function graphFromInvestigation(graph) {
    const nodes = (graph?.entities || []).slice(0, 80).map((entity) => ({
      data: {
        id: `entity:${entity.id}`,
        label: entity.label || entity.key || String(entity.id),
        kind: entity.entity_type || "entity",
        status: entity.trust_level === "verified" ? "online" : "unknown",
      },
    }));
    const seen = new Set(nodes.map((node) => node.data.id));
    const edges = (graph?.relationships || []).slice(0, 120).flatMap((rel) => {
      const source = `entity:${rel.source_entity_id}`;
      const target = `entity:${rel.target_entity_id}`;
      if (!seen.has(source) || !seen.has(target)) return [];
      return [{
        data: {
          id: `rel:${rel.id}`,
          source,
          target,
          label: rel.relationship_type || rel.reason_code || "related",
        },
      }];
    });
    return nodes.length ? { nodes, edges } : seed;
  }

  function showNodeEvidence(data) {
    ui.selectedTitle.textContent = data.label || data.id;
    setBadge(ui.selectedKind, data.kind || "node", data.status === "degraded" ? "warn" : "neutral");
    const facts = [
      ["Node id", data.id],
      ["Kind", data.kind || "unknown"],
      ["Status", data.status || "unknown"],
    ];
    const related = latestLogs.filter((row) => {
      const host = row.hostname || row.host || "";
      return data.label && String(host).toLowerCase() === String(data.label).toLowerCase();
    }).slice(0, 4);
    clear(ui.evidenceList);
    facts.forEach(([label, value]) => {
      const item = text("article", "evidence-item", "");
      item.append(text("span", "meta-label", label));
      item.append(text("p", null, value));
      ui.evidenceList.append(item);
    });
    related.forEach((row) => {
      const item = text("article", "evidence-item", "");
      item.append(text("span", "meta-label", row.severity || "log"));
      item.append(text("p", null, row.message || row.raw || "No message"));
      ui.evidenceList.append(item);
    });
  }

  function renderTimeline(rows) {
    clear(ui.timeline);
    if (!rows.length) {
      ui.timeline.append(text("p", "muted", "No log rows returned for the current query."));
      return;
    }
    rows.slice(0, 20).forEach((row) => {
      const entry = text("article", "timeline-row", "");
      entry.append(text("time", null, compactTime(row.timestamp || row.received_at)));
      entry.append(text("span", "severity", row.severity || "unknown"));
      const message = text("span", "message", row.message || row.raw || "");
      entry.append(message);
      ui.timeline.append(entry);
    });
  }

  function compactTime(value) {
    if (!value) return "--";
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return String(value).slice(0, 19);
    return date.toISOString().slice(11, 19);
  }

  function renderAnswer(title, body, tone = "violet") {
    clear(ui.answerStack);
    const card = text("article", "answer-card", "");
    const badge = text("span", `badge ${tone}`, tone);
    card.append(badge);
    card.append(text("h2", null, title));
    card.append(text("p", null, body));
    ui.answerStack.append(card);
  }

  function renderClaims(prompt, envelope) {
    clear(ui.answerStack);
    const claims = envelope?.result?.claims || [];
    if (!claims.length) {
      renderAnswer(prompt, "Cortex returned no claims for this investigation.", "warn");
      return;
    }
    claims.slice(0, 6).forEach((claim) => {
      const tone = claim.claim_type === "open_question" ? "warn" : "violet";
      const card = text("article", "answer-card", "");
      card.append(text("span", `badge ${tone}`, claim.claim_type || "claim"));
      card.append(text("h2", null, claim.title || prompt));
      card.append(text("p", null, claim.summary || ""));
      ui.answerStack.append(card);
    });
  }

  async function checkV1Compatibility() {
    try {
      const payload = await apiGet("/api/v1/investigation/version");
      if (payload.ui_version && payload.ui_version !== appVersion) {
        setV1State("Version skew", `Server expects ${payload.ui_version}`, "warn");
      } else {
        setV1State("Compatible", "Investigation API v1 is available", "");
      }
    } catch (error) {
      setV1State("Unavailable", "/api/v1 is not mounted; Ask + Explain is disabled", "warn");
    }
  }

  async function refresh() {
    if (!bearerToken) {
      renderAnswer("Bearer token required", "Enter CORTEX_API_TOKEN to load live API data.", "warn");
      return;
    }

    try {
      await checkV1Compatibility();
      const [version, stats, hosts, tail] = await Promise.all([
        apiGet("/api/version"),
        apiGet("/api/stats"),
        apiGet("/api/hosts"),
        apiGet("/api/tail?n=25"),
      ]);

      latestHosts = hosts.hosts || hosts.items || [];
      latestLogs = tail.logs || tail.entries || tail.items || [];
      ui.serverVersion.textContent = version.version || "Unknown";
      ui.schemaVersion.textContent = `Schema ${version.schema_version ?? "--"}`;
      ui.hostCount.textContent = String(latestHosts.length);
      ui.logCount.textContent = String(stats.total_logs ?? stats.total ?? "--");
      setBadge(ui.logStatus, "Live", "success");
      updateGraph(graphFromHosts(latestHosts));
      renderTimeline(latestLogs);
      renderAnswer("Live workspace connected", "Cortex returned version, stats, hosts, and recent log evidence. Ask a question to run the /api/v1 Ask + Explain workflow.", "success");
    } catch (error) {
      setBadge(ui.logStatus, "Error", "error");
      renderAnswer("Backend unavailable", error.message, "error");
    }
  }

  async function ask(query) {
    if (!query.trim()) return;
    if (!bearerToken) {
      renderAnswer("Bearer token required", "Connect before asking Cortex for live evidence.", "warn");
      return;
    }
    try {
      const envelope = await apiPost("/api/v1/investigations/ask", { prompt: query });
      latestLogs = envelope.result?.logs || [];
      renderClaims(query, envelope);
      renderTimeline(latestLogs);
      updateGraph(graphFromInvestigation(envelope.result?.graph));
      if (envelope.metadata?.partial) {
        setBadge(ui.logStatus, "Partial", "warn");
      } else {
        setBadge(ui.logStatus, "Explained", "success");
      }
    } catch (error) {
      renderAnswer("Ask failed", error.message, "error");
    }
  }

  ui.tokenForm.addEventListener("submit", (event) => {
    event.preventDefault();
    bearerToken = new FormData(ui.tokenForm).get("token")?.toString().trim() || "";
    refresh();
  });

  ui.clearToken.addEventListener("click", () => {
    bearerToken = "";
    ui.tokenForm.reset();
    latestHosts = [];
    latestLogs = [];
    updateGraph(seed);
    renderTimeline([]);
    setBadge(ui.logStatus, "Preview", "neutral");
    setV1State("Disconnected", "Enter a bearer token to check /api/v1", "warn");
    renderAnswer("Bearer token cleared", "The token was removed from memory.", "warn");
  });

  ui.askForm.addEventListener("submit", (event) => {
    event.preventDefault();
    ask(new FormData(ui.askForm).get("query")?.toString() || "");
  });

  ui.fitGraph.addEventListener("click", () => cy?.fit(undefined, 36));
  ui.refresh.addEventListener("click", refresh);

  initGraph();
})();
