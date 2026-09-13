"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
const http = require("node:http");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { download } = require("../lib/download");

test("download bounds redirects, idle, total lifetime, size and partial responses", async () => {
  const server = http.createServer((req, res) => {
    if (req.url === "/redirect") { res.writeHead(302, { location: "/ok" }); res.end(); }
    else if (req.url === "/loop") { res.writeHead(302, { location: "/loop" }); res.end(); }
    else if (req.url === "/idle") { /* intentionally no response */ }
    else if (req.url === "/drip") { res.writeHead(200); const timer = setInterval(() => res.write("x"), 5); res.on("close", () => clearInterval(timer)); }
    else if (req.url === "/partial") { res.writeHead(200, { "content-length": 100 }); res.write("x"); setImmediate(() => res.destroy()); }
    else { res.end("hello"); }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "cortex-download-"));
  const dest = path.join(dir, "asset");
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    await download(base + "/redirect", dest);
    assert.equal(fs.readFileSync(dest, "utf8"), "hello");
    for (const [route, options] of [["loop", { maxRedirects: 2 }], ["idle", { idleTimeoutMs: 30 }], ["drip", { timeoutMs: 60 }], ["ok", { maxBytes: 2 }], ["partial", {}]]) {
      await assert.rejects(download(base + "/" + route, dest, options));
      assert.equal(fs.existsSync(dest), false, route + " left partial file");
    }
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
