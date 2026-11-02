#!/usr/bin/env node
// Set the app version in every place that carries it, consistently:
//   package.json, package-lock.json (top level + packages[""]), src-tauri/Cargo.toml,
//   src-tauri/Cargo.lock (the brainitive-desktop entry, edited in place so it works offline),
//   src-tauri/tauri.conf.json.
//
//   node scripts/bump-version.mjs 1.0.0              write the files
//   node scripts/bump-version.mjs 1.0.0 --dry-run    show what would change, write nothing
//   node scripts/bump-version.mjs 1.0.0 --check      exit 1 unless every file already has it
//   --root <dir>                                     another checkout (used by the test)
//
// Only the version lines change; formatting of the files is kept. Run `cargo check --locked`
// afterwards (CI does) to confirm Cargo.lock is in step.
import { readFileSync, writeFileSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SEMVER = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$/;

/** Replace `re`'s single match (group 1 = text before the version, group 3 = after). */
function replaceOne(text, re, version, label) {
  const m = text.match(re);
  if (!m) throw new Error(`${label}: version line not found`);
  return { next: text.replace(re, `$1${version}$3`), current: m[2] };
}

export const targets = [
  {
    file: "package.json",
    edit: (t, v) => replaceOne(t, /^(  "version": ")([^"]+)(")/m, v, "package.json"),
  },
  {
    file: "package-lock.json",
    edit: (t, v) => {
      // the first two "version" lines: the root, then packages[""]
      let current;
      let n = 0;
      const next = t.replace(/^(\s*"version": ")([^"]+)(")/gm, (all, a, cur, c) => {
        if (n++ > 1) return all;
        current ??= cur;
        if (n === 2 && cur !== current) throw new Error("package-lock.json: root versions differ");
        return `${a}${v}${c}`;
      });
      if (n < 2) throw new Error("package-lock.json: version lines not found");
      return { next, current };
    },
  },
  {
    file: "src-tauri/Cargo.toml",
    edit: (t, v) => replaceOne(t, /^(\[package\][^[]*?\nversion = ")([^"]+)(")/, v, "Cargo.toml"),
  },
  {
    file: "src-tauri/Cargo.lock",
    edit: (t, v) =>
      replaceOne(t, /^(name = "brainitive-desktop"\nversion = ")([^"]+)(")/m, v, "Cargo.lock"),
  },
  {
    file: "src-tauri/tauri.conf.json",
    edit: (t, v) => replaceOne(t, /^(  "version": ")([^"]+)(")/m, v, "tauri.conf.json"),
  },
];

/** Returns [{file, from, to}] and writes unless `write` is false. */
export function bump(root, version, write) {
  if (!SEMVER.test(version)) throw new Error(`"${version}" is not a semantic version (1.2.3)`);
  const results = [];
  const pending = [];
  for (const t of targets) {
    const path = join(root, t.file);
    const { next, current } = t.edit(readFileSync(path, "utf8"), version);
    results.push({ file: t.file, from: current, to: version });
    pending.push([path, next]);
  }
  if (write) for (const [p, c] of pending) writeFileSync(p, c);
  return results;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const flag = (f) => args.includes(f);
  const ri = args.indexOf("--root");
  const root = ri >= 0 ? resolve(args[ri + 1]) : resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const version = args.find((a, i) => !a.startsWith("--") && args[i - 1] !== "--root");
  if (!version) {
    console.error("usage: node scripts/bump-version.mjs <version> [--dry-run | --check] [--root <dir>]");
    process.exit(2);
  }
  try {
    const res = bump(root, version, !flag("--dry-run") && !flag("--check"));
    for (const r of res) console.log(`${r.from === version ? "=" : "~"} ${r.file}: ${r.from} -> ${r.to}`);
    if (flag("--check") && res.some((r) => r.from !== version)) {
      console.error(`not every file is at ${version}`);
      process.exit(1);
    }
    if (flag("--dry-run")) console.log("(dry run, nothing written)");
  } catch (e) {
    console.error(String(e.message ?? e));
    process.exit(2);
  }
}
