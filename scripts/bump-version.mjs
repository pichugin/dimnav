#!/usr/bin/env node
/**
 * Bump the app version everywhere it is written down.
 *
 * The workspace Cargo.toml is the single source of truth: src-tauri inherits it
 * with `version.workspace = true`, and tauri.conf.json deliberately omits
 * `version` so the bundler falls back to the crate version. npm's manifests are
 * the only copies that cannot inherit, so this script drags them along.
 *
 *   node scripts/bump-version.mjs 0.2.0     # or: npm run bump 0.2.0
 *
 * Prints the git commands to run; it never commits or tags on your behalf.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

const version = process.argv[2];
if (!version) {
  console.error("usage: node scripts/bump-version.mjs <version>");
  process.exit(1);
}
// Plain semver only. Tauri rejects anything else, and on macOS the value becomes
// CFBundleShortVersionString, where a malformed string fails at notarization
// rather than at build time — far too late to notice.
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`not a semver version: ${version} (expected e.g. 0.2.0)`);
  process.exit(1);
}

/** Replace an exact string once, failing loudly if the anchor moved. */
function edit(relPath, find, replace) {
  const path = join(ROOT, relPath);
  const text = readFileSync(path, "utf8");
  const hits = text.split(find).length - 1;
  if (hits !== 1) {
    console.error(`${relPath}: expected exactly one match for ${JSON.stringify(find)}, found ${hits}`);
    process.exit(1);
  }
  writeFileSync(path, text.replace(find, replace));
  console.log(`  ${relPath}`);
}

function currentCargoVersion() {
  const text = readFileSync(join(ROOT, "Cargo.toml"), "utf8");
  const match = text.match(/\[workspace\.package\][\s\S]*?\nversion = "([^"]+)"/);
  if (!match) {
    console.error("could not find version in [workspace.package] of Cargo.toml");
    process.exit(1);
  }
  return match[1];
}

const from = currentCargoVersion();
if (from === version) {
  console.error(`already at ${version}`);
  process.exit(1);
}

console.log(`${from} -> ${version}`);
edit("Cargo.toml", `\nversion = "${from}"`, `\nversion = "${version}"`);
edit("package.json", `"version": "${from}"`, `"version": "${version}"`);

// package-lock.json carries the version twice: the top-level field and the root
// package entry. Both must move or `npm ci` warns the lockfile is out of date.
const lockPath = join(ROOT, "package-lock.json");
const lock = JSON.parse(readFileSync(lockPath, "utf8"));
lock.version = version;
if (lock.packages?.[""]) lock.packages[""].version = version;
writeFileSync(lockPath, JSON.stringify(lock, null, 2) + "\n");
console.log("  package-lock.json");

// Cargo.lock records the workspace crates' versions too; refreshing it keeps the
// release commit self-consistent without a full rebuild.
console.log("\nNext:");
console.log("  cargo check --workspace   # refresh Cargo.lock");
console.log(`  git commit -am "release: v${version}"`);
console.log(`  git tag v${version} && git push origin main --tags`);
