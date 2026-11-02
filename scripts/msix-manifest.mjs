#!/usr/bin/env node
// Renders the MSIX manifest of the Microsoft Store build (BRAI-442) from
// src-tauri/msix/AppxManifest.xml.tmpl.
//
//   node scripts/msix-manifest.mjs <out AppxManifest.xml> [X.Y.Z]
//
// The version defaults to src-tauri/tauri.conf.json's and becomes X.Y.Z.0: the Store reserves
// the fourth part (it must be 0) and needs every submission to be higher than the last one.
// Pre-release versions (1.0.0-beta.1) have no MSIX form and are refused.
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const templatePath = join(repo, "src-tauri", "msix", "AppxManifest.xml.tmpl");

/** `1.2.3` → `1.2.3.0`; each part 0…65535 (MSIX version parts are 16-bit). */
export function msixVersion(version) {
  const m = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec(String(version).trim());
  if (!m) {
    throw new Error(
      `"${version}" has no MSIX form: the Store needs a plain X.Y.Z version (no pre-release or build part)`,
    );
  }
  const parts = m.slice(1, 4).map(Number);
  if (parts.some((p) => p > 65535)) throw new Error(`"${version}": MSIX version parts are at most 65535`);
  return `${parts.join(".")}.0`;
}

/** The template with its placeholders filled; any placeholder left over is an error. */
export function render(template, { version }) {
  const out = template.replaceAll("{{VERSION}}", msixVersion(version));
  const left = out.match(/\{\{[A-Z_]+\}\}/);
  if (left) throw new Error(`unknown placeholder ${left[0]} in the manifest template`);
  return out;
}

export function appVersion() {
  return JSON.parse(readFileSync(join(repo, "src-tauri", "tauri.conf.json"), "utf8")).version;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [out, version = appVersion()] = process.argv.slice(2);
  if (!out) {
    console.error("usage: node scripts/msix-manifest.mjs <out AppxManifest.xml> [X.Y.Z]");
    process.exit(2);
  }
  try {
    const xml = render(readFileSync(templatePath, "utf8"), { version });
    mkdirSync(dirname(resolve(out)), { recursive: true });
    writeFileSync(out, xml);
    console.log(`${out}: version ${msixVersion(version)}`);
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}
