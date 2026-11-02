// node --test scripts/
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, copyFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { bump, targets } from "./bump-version.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function sandbox() {
  const dir = mkdtempSync(join(tmpdir(), "bump-"));
  mkdirSync(join(dir, "src-tauri"));
  for (const t of targets) copyFileSync(join(repo, t.file), join(dir, t.file));
  return dir;
}

test("dry run changes nothing, write changes only version lines", () => {
  const dir = sandbox();
  try {
    const before = targets.map((t) => readFileSync(join(dir, t.file), "utf8"));
    bump(dir, "9.8.7", false);
    assert.deepEqual(targets.map((t) => readFileSync(join(dir, t.file), "utf8")), before);
    bump(dir, "9.8.7", true);
    targets.forEach((t, i) => {
      const after = readFileSync(join(dir, t.file), "utf8");
      const a = before[i].split("\n");
      const b = after.split("\n");
      assert.equal(a.length, b.length, t.file);
      const changed = a.filter((l, k) => l !== b[k]);
      assert.ok(changed.length >= 1 && changed.length <= 2, `${t.file}: ${changed.length} lines`);
      assert.ok(b.some((l) => l.includes('"9.8.7"')), t.file);
    });
    assert.ok(bump(dir, "9.8.7", false).every((r) => r.from === "9.8.7"));
  } finally {
    rmSync(dir, { recursive: true });
  }
});

test("rejects a bad version", () => {
  assert.throws(() => bump(repo, "v1.0", false), /semantic version/);
});
