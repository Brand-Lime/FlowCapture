#!/usr/bin/env node
// Builds the FlowCapture Companion browser extension packages from `browser-extension/`:
//
//   dist-extensions/chromium/                              Chrome, Edge, Brave, Opera (unpacked)
//   dist-extensions/firefox/                               Firefox (unpacked)
//   dist-extensions/flowcapture-companion-chromium-<v>.zip upload: Chrome Web Store, Edge Add-ons, Opera add-ons
//   dist-extensions/flowcapture-companion-firefox-<v>.zip  upload: addons.mozilla.org (signing)
//
// The sources are shared: only the manifest differs (Firefox runs the background script as an
// event page and needs a fixed add-on id, which FlowCapture also uses to find it in the profiles).
//
// Usage: node scripts/build-browser-extensions.mjs

import { mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateRawSync } from "node:zlib";

/** Firefox add-on id: must match FIREFOX_EXTENSION_ID in src-tauri/src/platform/browser_extensions.rs. */
export const FIREFOX_EXTENSION_ID = "companion@flowcapture.app";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const sourceDir = join(repoRoot, "browser-extension");
const outDir = join(repoRoot, "dist-extensions");
/** Files of the source folder that are not part of the extension. */
const EXCLUDED = new Set(["README.md"]);

const baseManifest = JSON.parse(readFileSync(join(sourceDir, "manifest.json"), "utf8").replace(/^﻿/, ""));

const targets = {
  chromium: baseManifest,
  firefox: {
    ...baseManifest,
    background: { scripts: [baseManifest.background.service_worker] },
    browser_specific_settings: {
      gecko: {
        id: FIREFOX_EXTENSION_ID,
        // Manifest V3 with background scripts.
        strict_min_version: "115.0",
        // Clicks go only to FlowCapture on this computer (127.0.0.1): nothing is collected.
        data_collection_permissions: { required: ["none"] },
      },
    },
  },
};

function listFiles(dir) {
  const files = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) files.push(...listFiles(path));
    else files.push(path);
  }
  return files;
}

// --- Minimal zip writer (deflate), no dependencies ------------------------------------------------
const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function zip(entries) {
  const now = new Date();
  const time = (now.getHours() << 11) | (now.getMinutes() << 5) | (now.getSeconds() >> 1);
  const date = ((now.getFullYear() - 1980) << 9) | ((now.getMonth() + 1) << 5) | now.getDate();
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const { name, data } of entries) {
    const nameBytes = Buffer.from(name, "utf8");
    const compressed = deflateRawSync(data, { level: 9 });
    const crc = crc32(data);

    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x0800, 6); // UTF-8 names
    local.writeUInt16LE(8, 8); // deflate
    local.writeUInt16LE(time, 10);
    local.writeUInt16LE(date, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(compressed.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    locals.push(local, nameBytes, compressed);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt16LE(8, 10);
    central.writeUInt16LE(time, 12);
    central.writeUInt16LE(date, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(compressed.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt32LE(offset, 42);
    centrals.push(central, nameBytes);

    offset += local.length + nameBytes.length + compressed.length;
  }
  const centralSize = centrals.reduce((sum, part) => sum + part.length, 0);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(centralSize, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, ...centrals, end]);
}

// --- Build ----------------------------------------------------------------------------------------
rmSync(outDir, { recursive: true, force: true });
const sourceFiles = listFiles(sourceDir).filter((path) => {
  const name = relative(sourceDir, path).replaceAll("\\", "/");
  return name !== "manifest.json" && !EXCLUDED.has(name);
});

for (const [target, manifest] of Object.entries(targets)) {
  const entries = [{ name: "manifest.json", data: Buffer.from(JSON.stringify(manifest, null, 2) + "\n", "utf8") }];
  for (const path of sourceFiles) {
    entries.push({ name: relative(sourceDir, path).replaceAll("\\", "/"), data: readFileSync(path) });
  }

  const folder = join(outDir, target);
  for (const { name, data } of entries) {
    mkdirSync(dirname(join(folder, name)), { recursive: true });
    writeFileSync(join(folder, name), data);
  }
  const archive = join(outDir, `flowcapture-companion-${target}-${manifest.version}.zip`);
  writeFileSync(archive, zip(entries));
  console.log(`${target.padEnd(9)} ${relative(repoRoot, folder)}  +  ${relative(repoRoot, archive)}`);
}
