#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const args = new Map();
for (let i = 2; i < process.argv.length; i += 1) {
  const key = process.argv[i];
  if (!key.startsWith("--")) continue;
  args.set(key.slice(2), process.argv[i + 1]);
  i += 1;
}

const version = cleanVersion(required("version"));
const tag = args.get("tag") ?? `v${version}`;
const repo = args.get("repo") ?? process.env.GITHUB_REPOSITORY ?? "caork/aka";
const dir = args.get("dir") ?? "dist";
const out = args.get("out") ?? path.join(dir, "latest.json");
const publishedAt = args.get("published-at") ?? new Date().toISOString();
const releaseUrl = args.get("release-url") ?? `https://github.com/${repo}/releases/tag/${tag}`;
const baseUrl =
  args.get("base-url") ??
  `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}`;

const files = fs.existsSync(dir) ? fs.readdirSync(dir).sort() : [];
const checksums = readChecksums(path.join(dir, "SHA256SUMS"));

const assets = [];
for (const name of files) {
  const fullPath = path.join(dir, name);
  if (!fs.statSync(fullPath).isFile()) continue;
  const info = assetInfo(name);
  if (!info) continue;
  assets.push({
    ...info,
    name,
    url: `${baseUrl}/${encodeURIComponent(name)}`,
    size: fs.statSync(fullPath).size,
    sha256: checksums.get(name),
  });
}

const manifest = {
  version,
  tag,
  releaseUrl,
  publishedAt,
  notes: `AKA ${version}`,
  platforms: Object.fromEntries(assets.map((asset) => [asset.platform, asset])),
  assets,
};

fs.mkdirSync(path.dirname(out), { recursive: true });
fs.writeFileSync(out, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`wrote ${out}`);

function required(name) {
  const value = args.get(name);
  if (!value) {
    console.error(`missing --${name}`);
    process.exit(1);
  }
  return value;
}

function cleanVersion(value) {
  return value.trim().replace(/^aka-desktop-/i, "").replace(/^v/i, "");
}

function assetInfo(name) {
  if (/aka-desktop-.+-(aarch64|x86_64)-apple-darwin\.dmg$/.test(name)) {
    return { platform: "macos", kind: "dmg", label: "macOS DMG" };
  }
  if (/aka-desktop-.+-x86_64-pc-windows-msvc-setup\.exe$/.test(name)) {
    return { platform: "windows", kind: "exe", label: "Windows EXE" };
  }
  return null;
}

function readChecksums(file) {
  const out = new Map();
  if (!fs.existsSync(file)) return out;
  for (const line of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    const match = line.match(/^([a-f0-9]{64})\s+\*?(.+)$/i);
    if (match) out.set(match[2], match[1].toLowerCase());
  }
  return out;
}
