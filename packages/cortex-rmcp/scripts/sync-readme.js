#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");

const packageRoot = path.resolve(__dirname, "..");
const repoRoot = path.resolve(__dirname, "..", "..", "..");
const args = process.argv.slice(2);
if (args.some((arg) => arg !== "--check")) {
  console.error("Usage: node scripts/sync-readme.js [--check]");
  process.exit(1);
}
const check = args.includes("--check");

function syncFile(name) {
  const source = path.join(repoRoot, name);
  const destination = path.join(packageRoot, name);
  const data = fs.readFileSync(source);
  const isLink = fs.existsSync(destination)
    ? fs.lstatSync(destination).isSymbolicLink()
    : (() => {
        try { return fs.lstatSync(destination).isSymbolicLink(); }
        catch (error) { if (error.code === "ENOENT") return false; throw error; }
      })();
  if (!isLink && fs.existsSync(destination) && data.equals(fs.readFileSync(destination))) {
    return;
  }
  if (check) {
    console.error(`package mirror is stale or missing: ${path.relative(repoRoot, destination)}`);
    process.exitCode = 1;
    return;
  }
  if (isLink) fs.unlinkSync(destination);
  fs.writeFileSync(destination, data);
}

syncFile("README.md");
for (const entry of fs.readdirSync(repoRoot).sort()) {
  const source = path.join(repoRoot, entry);
  if (/^licen[cs]e/i.test(entry) && fs.statSync(source).isFile()) {
    syncFile(entry);
  }
}
