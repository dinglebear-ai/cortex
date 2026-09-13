"use strict";
const fs = require("node:fs");
const http = require("node:http");
const https = require("node:https");
const { Transform } = require("node:stream");
const { pipeline } = require("node:stream/promises");

async function download(url, destination, options = {}) {
  const { timeoutMs = 120000, idleTimeoutMs = 15000, maxBytes = 128 * 1024 * 1024, maxRedirects = 8 } = options;
  const controller = new AbortController();
  const deadline = setTimeout(() => controller.abort(new Error("download deadline exceeded")), timeoutMs);
  try {
    for (let redirects = 0; ; redirects++) {
      const parsed = new URL(url);
      if (!["http:", "https:"].includes(parsed.protocol)) throw new Error("unsupported download protocol");
      const response = await new Promise((resolve, reject) => {
        const request = (parsed.protocol === "http:" ? http : https).get(parsed, { signal: controller.signal }, resolve);
        request.setTimeout(idleTimeoutMs, () => request.destroy(new Error("download idle timeout")));
        request.once("error", reject);
      });
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        const location = response.headers.location;
        response.destroy();
        if (!location || redirects >= maxRedirects) throw new Error("download redirect limit exceeded or missing location");
        url = new URL(location, parsed).toString();
        continue;
      }
      if (response.statusCode !== 200 || Number(response.headers["content-length"] || 0) > maxBytes) {
        response.destroy();
        throw new Error(`download rejected: status ${response.statusCode} or excessive content length`);
      }
      let received = 0;
      const bounded = new Transform({ transform(chunk, encoding, callback) {
        received += chunk.length;
        callback(received > maxBytes ? new Error("download byte limit exceeded") : null, chunk);
      }});
      await pipeline(response, bounded, fs.createWriteStream(destination, { mode: 0o600 }), { signal: controller.signal });
      return;
    }
  } catch (error) {
    fs.rmSync(destination, { force: true });
    throw error;
  } finally {
    clearTimeout(deadline);
  }
}
module.exports = { download };
