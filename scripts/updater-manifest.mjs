#!/usr/bin/env node
// Writes the updater manifest (`latest.json`, Tauri's static format) for one tagged release
// (BRAI-373). Used by the release job of .github/workflows/build.yml; no dependencies.
//
//   node scripts/updater-manifest.mjs <dir> <tag> <owner/repo> [notes] > latest.json
//
// <dir> holds the signed updater bundles of the build (`*.app.tar.gz`, `*-setup.exe`,
// `*.AppImage`, `*.deb`, each with its `.sig`). Bundles without a `.sig` are left out. The tag
// must match the version in src-tauri/tauri.conf.json. Exit 2 = nothing signed (no manifest).

import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const [dir, tag, repo, notes = ''] = process.argv.slice(2);
if (!dir || !tag || !repo) {
    console.error('usage: updater-manifest.mjs <dir> <tag> <owner/repo> [notes]');
    process.exit(1);
}

const version = tag.replace(/^v/, '');
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
    console.error(`not a release tag: ${tag}`);
    process.exit(1);
}
const configPath = new URL('../src-tauri/tauri.conf.json', import.meta.url);
const configured = JSON.parse(readFileSync(configPath, 'utf8')).version;
if (configured !== version) {
    console.error(`tag ${tag} does not match tauri.conf.json version ${configured}`);
    process.exit(1);
}
if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) {
    console.error(`not a GitHub repository: ${repo}`);
    process.exit(1);
}

// file name → the platform keys the updater looks up ({os}-{arch}-{installer}, then {os}-{arch})
const kinds = [
    // the universal macOS build serves both architectures
    { test: (f) => f.endsWith('.app.tar.gz'), keys: ['darwin-aarch64', 'darwin-x86_64', 'darwin-aarch64-app', 'darwin-x86_64-app'] },
    { test: (f) => f.endsWith('-setup.exe'), keys: ['windows-x86_64', 'windows-x86_64-nsis'] },
    { test: (f) => f.endsWith('.AppImage'), keys: ['linux-x86_64', 'linux-x86_64-appimage'] },
    { test: (f) => f.endsWith('.deb'), keys: ['linux-x86_64-deb'] },
];

const platforms = {};
for (const file of readdirSync(dir).sort()) {
    const kind = kinds.find((k) => k.test(file));
    const sigPath = join(dir, `${file}.sig`);
    if (!kind || !existsSync(sigPath)) continue;
    const signature = readFileSync(sigPath, 'utf8').trim();
    const url = `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(file)}`;
    for (const key of kind.keys) {
        if (platforms[key]) {
            console.error(`two bundles for ${key}: ${file}`);
            process.exit(1);
        }
        platforms[key] = { signature, url };
    }
}

if (Object.keys(platforms).length === 0) {
    console.error('no signed updater bundles: no manifest');
    process.exit(2);
}

const manifest = { version, notes, pub_date: new Date().toISOString(), platforms };
process.stdout.write(`${JSON.stringify(manifest, null, 2)}\n`);
