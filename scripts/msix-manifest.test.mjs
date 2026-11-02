// node --test scripts/   (BRAI-442: the Microsoft Store package)
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { msixVersion, render, repo, templatePath, appVersion } from "./msix-manifest.mjs";
import { ASSETS, decodePng, encodePng, fileName, logo, writeAssets } from "./msix-assets.mjs";

const template = readFileSync(templatePath, "utf8");
const listing = readFileSync(join(repo, "docs", "MICROSOFT-STORE-LISTING.md"), "utf8");
const rust = readFileSync(join(repo, "src-tauri", "src", "msstore", "mod.rs"), "utf8");

/** A value of the Packages table in MICROSOFT-STORE-LISTING.md § 7: | Field | `value` |. */
function listed(field) {
  const row = listing.split("\n").find((l) => l.startsWith(`| ${field} |`));
  assert.ok(row, `no row "${field}" in docs/MICROSOFT-STORE-LISTING.md`);
  return /`([^`]+)`/.exec(row)[1];
}

/** `pub const NAME: &str = "value";` in src-tauri/src/msstore/mod.rs. */
function rustConst(name) {
  const m = new RegExp(`pub const ${name}: &str = "([^"]+)";`).exec(rust);
  assert.ok(m, `no const ${name} in msstore/mod.rs`);
  return m[1];
}

/** `attribute="value"` of the first element `tag` in the template. */
function attr(tag, attribute) {
  const element = new RegExp(`<${tag}\\b[^>]*>`, "s").exec(template);
  assert.ok(element, `no <${tag}> in the template`);
  const m = new RegExp(`\\b${attribute}="([^"]*)"`).exec(element[0]);
  assert.ok(m, `<${tag}> has no ${attribute}`);
  return m[1];
}

test("versions map to X.Y.Z.0", () => {
  assert.equal(msixVersion("1.2.3"), "1.2.3.0");
  assert.equal(msixVersion("0.1.0"), "0.1.0.0");
  assert.equal(msixVersion("10.20.65535"), "10.20.65535.0");
  for (const bad of ["1.0.0-beta.1", "1.0.0+build.5", "1.0", "1.2.3.4", "01.2.3", "1.2.65536", "v1.2.3", ""]) {
    assert.throws(() => msixVersion(bad), undefined, bad);
  }
});

test("the rendered manifest has the app's version and no placeholder left", () => {
  const xml = render(template, { version: "1.2.3" });
  assert.match(xml, /<Identity\b[^>]*\bVersion="1\.2\.3\.0"/s);
  assert.doesNotMatch(xml, /\{\{/);
  assert.throws(() => render(`${template}{{OTHER}}`, { version: "1.2.3" }), /OTHER/);
  // the default is tauri.conf.json's version: a plain X.Y.Z has an MSIX form; a pre-release
  // (1.0.0-beta.2) has none, and the msstore job skips such tags
  if (appVersion().includes("-")) {
    assert.throws(() => msixVersion(appVersion()), /no MSIX form/);
  } else {
    assert.equal(msixVersion(appVersion()), `${appVersion()}.0`);
  }
});

test("the identity is exactly the one in MICROSOFT-STORE-LISTING.md and msstore/mod.rs", () => {
  assert.equal(attr("Identity", "Name"), listed("Package/Identity/Name"));
  assert.equal(attr("Identity", "Publisher"), listed("Package/Identity/Publisher"));
  assert.equal(/<PublisherDisplayName>([^<]+)</.exec(template)[1], listed("PublisherDisplayName"));
  assert.equal(attr("Identity", "Name"), rustConst("PACKAGE_NAME"));
  assert.equal(attr("Identity", "Publisher"), rustConst("PUBLISHER"));
  assert.equal(listed("PublisherDisplayName"), rustConst("PUBLISHER_DISPLAY_NAME"));
  assert.equal(listed("Package Family Name"), rustConst("PACKAGE_FAMILY_NAME"));
  assert.equal(listed("Store ID"), rustConst("STORE_ID"));
  assert.equal(attr("Identity", "ProcessorArchitecture"), "x64");
});

test("the application, startup task and protocol match the app", () => {
  assert.equal(attr("Application", "Id"), rustConst("APPLICATION_ID"));
  assert.equal(attr("Application", "EntryPoint"), "Windows.FullTrustApplication");
  assert.equal(attr("desktop:StartupTask", "TaskId"), rustConst("STARTUP_TASK_ID"));
  assert.equal(attr("desktop:StartupTask", "Enabled"), "false", "off until the user turns it on");
  const schemes = JSON.parse(readFileSync(join(repo, "src-tauri", "tauri.conf.json"), "utf8")).plugins["deep-link"]
    .desktop.schemes;
  assert.deepEqual([attr("uap3:Protocol", "Name")], schemes);
  // the URI as the only argument: what tauri-plugin-deep-link reads on Windows
  assert.equal(attr("uap3:Protocol", "Parameters"), "&quot;%1&quot;");
  assert.deepEqual(
    { ...["MinVersion", "MaxVersionTested"].reduce((o, a) => ({ ...o, [a]: attr("TargetDeviceFamily", a) }), {}) },
    { MinVersion: "10.0.17763.0", MaxVersionTested: "10.0.26100.0" },
  );
  for (const capability of ['rescap:Capability Name="runFullTrust"', 'Capability Name="internetClient"', 'DeviceCapability Name="microphone"']) {
    assert.ok(template.includes(`<${capability}`), capability);
  }
});

test("every logo the manifest names has generated assets", () => {
  const named = [...template.matchAll(/Assets\\([A-Za-z0-9]+)\.png/g)].map((m) => m[1]);
  assert.ok(named.length >= 5, named.join());
  const made = new Set(ASSETS.map(([base]) => base));
  for (const base of named) assert.ok(made.has(base), `no asset for ${base}`);
  // scale-100 of each is the size the manifest schema expects
  const size = Object.fromEntries(ASSETS.filter((a) => a[1] === "scale-100").map((a) => [a[0], [a[2], a[3]]]));
  assert.deepEqual(size.StoreLogo, [50, 50]);
  assert.deepEqual(size.Square44x44Logo, [44, 44]);
  assert.deepEqual(size.Square150x150Logo, [150, 150]);
  assert.deepEqual(size.Wide310x150Logo, [310, 150]);
});

test("the assets are made from the app icon at their sizes", () => {
  const out = mkdtempSync(join(tmpdir(), "msix-assets-"));
  try {
    const names = writeAssets(join(repo, "src-tauri", "icons", "icon.png"), out);
    assert.deepEqual(names.sort(), readdirSync(out).sort());
    for (const asset of ASSETS) {
      const img = decodePng(readFileSync(join(out, fileName(asset))));
      assert.deepEqual([img.width, img.height], [asset[2], asset[3]], fileName(asset));
      // transparent corners (no plate), and the icon drawn
      assert.equal(img.data[3], 0, `${fileName(asset)} corner`);
      const opaque = img.data.filter((_, i) => i % 4 === 3 && img.data[i] > 128).length;
      assert.ok(opaque > (img.width * img.height) / 20, `${fileName(asset)}: ${opaque} opaque pixels`);
    }
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});

test("PNGs survive a round trip and logos are centred", () => {
  const dot = { width: 2, height: 2, data: Buffer.from([255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 9, 9, 9, 0]) };
  assert.deepEqual(decodePng(encodePng(dot)), dot);
  const square = { width: 4, height: 4, data: Buffer.alloc(64, 255) };
  const wide = logo(square, 10, 4, 0.5);
  const alphaAt = (x, y) => wide.data[(y * 10 + x) * 4 + 3];
  assert.equal(alphaAt(0, 0), 0);
  assert.equal(alphaAt(4, 1), 255);
  assert.equal(alphaAt(5, 2), 255);
  assert.equal(alphaAt(9, 3), 0);
});
