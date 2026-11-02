// node --test scripts/   (codex review 11: platform signing secrets never reach the build)
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const workflow = (name) => readFileSync(join(repo, ".github", "workflows", name), "utf8");

/** The jobs of a workflow: name → its steps (name + full text). Plain line parsing of our layout. */
function jobs(text) {
  const out = new Map();
  const lines = text.split("\n");
  const start = lines.findIndex((l) => l === "jobs:");
  let job = null;
  let step = null;
  for (const line of lines.slice(start + 1)) {
    const jobHead = line.match(/^  ([a-z][a-z0-9-]*):\s*$/);
    if (jobHead) {
      job = { steps: [], text: "" };
      out.set(jobHead[1], job);
      step = null;
      continue;
    }
    if (!job) continue;
    // comments describe steps, they don't run: never part of a step's text
    if (/^\s*#/.test(line)) continue;
    job.text += line + "\n";
    if (/^      - /.test(line)) {
      const name = line.match(/^      - name: (.+)$/)?.[1] ?? line.trim();
      step = { name, text: "" };
      job.steps.push(step);
    }
    if (step) {
      // a later key of the step (`name:` after `uses:`) names it
      const named = line.match(/^        name: (.+)$/);
      if (named) step.name = named[1];
      step.text += line + "\n";
    }
  }
  return out;
}

const build = jobs(workflow("build.yml"));
const PLATFORM = /secrets\.(APPLE_|AZURE_)/;
const SIGNING_STEPS = new Set([
  "macOS signing + notarization",
  "Windows signing (app)",
  "Windows signing (installer)",
]);
const BUILDS = [/npx tauri build/, /npx tauri bundle/, /\bnpm (ci|run|install)\b/, /\bcargo (build|test|clippy|install|check)\b/, /- run: npm/];

test("the build steps get no secret at all", () => {
  for (const [jobName, job] of build) {
    for (const step of job.steps) {
      if (!BUILDS.some((re) => re.test(step.text))) continue;
      assert.doesNotMatch(step.text, /secrets\./, `${jobName} › ${step.name} builds and sees a secret`);
      assert.doesNotMatch(step.text, /\b(APPLE_|AZURE_|TAURI_SIGNING_)\w*:/, `${jobName} › ${step.name}`);
    }
  }
  // the build itself, specifically
  const tauri = build.get("build").steps.find((s) => s.name === "tauri build");
  assert.ok(tauri, "no tauri build step");
  assert.doesNotMatch(tauri.text, /env:/, "tauri build has no env at all");
});

test("Apple / Azure secrets reach only the signing steps, which build nothing", () => {
  const seen = new Set();
  for (const [jobName, job] of build) {
    for (const step of job.steps) {
      if (step.name === "Detect signing secrets") {
        // booleans only: every secret is compared with ''
        for (const m of step.text.matchAll(/secrets\.[A-Z0-9_]+(.{0,6})/g)) {
          assert.match(m[1], /^ != ''/, `${step.name}: ${m[0]}`);
        }
        continue;
      }
      if (!PLATFORM.test(step.text)) continue;
      assert.equal(jobName, "build");
      assert.ok(SIGNING_STEPS.has(step.name), `${step.name} gets a platform signing secret`);
      seen.add(step.name);
      for (const re of BUILDS) assert.doesNotMatch(step.text, re, `${step.name} must not build`);
      assert.doesNotMatch(step.text, /\bnpx\b|\bnode\b/, `${step.name} runs no npm package`);
    }
  }
  assert.deepEqual([...seen].sort(), [...SIGNING_STEPS].sort());
});

test("the signing steps come after the build they sign", () => {
  const names = build.get("build").steps.map((s) => s.name);
  const at = (n) => {
    const i = names.indexOf(n);
    assert.ok(i >= 0, `no step ${n}`);
    return i;
  };
  assert.ok(at("macOS signing + notarization") > at("tauri build"));
  assert.ok(at("Windows signing (app)") > at("tauri build"));
  assert.ok(at("tauri bundle (Windows)") > at("Windows signing (app)"));
  assert.ok(at("Windows signing (installer)") > at("tauri bundle (Windows)"));
  // the updater archive is made from the signed, stapled app
  assert.ok(at("Updater signing") > at("macOS signing + notarization"));
  assert.ok(at("Updater signing") > at("Windows signing (installer)"));
  // the signing tool is installed without secrets
  const tool = build.get("build").steps.find((s) => s.name === "Windows signing tool");
  assert.doesNotMatch(tool.text, /secrets\./);
});

test("the updater key reaches only the updater signing step", () => {
  for (const [, job] of build) {
    for (const step of job.steps) {
      if (step.name === "Detect signing secrets") continue;
      if (/secrets\.TAURI_SIGNING/.test(step.text)) assert.equal(step.name, "Updater signing");
    }
  }
});

test("the Store job has no secret, nothing goes to GITHUB_ENV, actions are pinned", () => {
  assert.doesNotMatch(build.get("msstore").text, /secrets\./);
  for (const file of ["build.yml", "ci.yml"]) {
    const text = workflow(file);
    assert.doesNotMatch(text, />>\s*"?\$\{?GITHUB_ENV/, `${file} writes to GITHUB_ENV`);
    for (const m of text.matchAll(/uses:\s*(\S+)/g)) {
      assert.match(m[1], /@[0-9a-f]{40}$/, `${file}: ${m[1]} is not pinned to a commit`);
    }
  }
});
