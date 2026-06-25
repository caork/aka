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
const requireUpdater = isTruthy(
  args.get("require-updater") ?? process.env.AKA_REQUIRE_UPDATER,
);

const files = fs.existsSync(dir) ? fs.readdirSync(dir).sort() : [];
const checksums = readChecksums(path.join(dir, "SHA256SUMS"));

const assets = [];
for (const name of files) {
  const fullPath = path.join(dir, name);
  if (!fs.statSync(fullPath).isFile()) continue;
  const info = assetInfo(name);
  if (!info) continue;
  const url = `${baseUrl}/${encodeURIComponent(name)}`;
  const signature = readSignature(dir, name, baseUrl);
  assets.push({
    ...info,
    name,
    url,
    downloadUrl: url,
    browserDownloadUrl: url,
    size: fs.statSync(fullPath).size,
    sha256: checksums.get(name),
    ...(signature ?? {}),
  });
}

if (assets.length === 0) {
  console.error(`no desktop update assets found in ${dir}`);
  process.exit(1);
}

const downloads = groupedDownloads(assets);
const platforms = updaterPlatforms(assets);
const manifest = {
  schemaVersion: 1,
  version,
  latestVersion: version,
  tag,
  releaseUrl,
  publishedAt,
  pub_date: publishedAt,
  notes: `AKA ${version}`,
  downloads,
  platforms,
  assets,
};

if (Object.keys(platforms).length === 0) {
  const message = `no Tauri updater platforms were generated; add .sig or .signature sidecars next to desktop updater assets in ${dir}`;
  if (requireUpdater) {
    console.error(`error: ${message}`);
    process.exit(1);
  }
  console.warn(`warning: ${message}`);
}

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

function isTruthy(value) {
  return /^(1|true|yes|required)$/i.test(String(value ?? "").trim());
}

function assetInfo(name) {
  const mac = name.match(/aka-desktop-.+-(aarch64|x86_64)-apple-darwin\.dmg$/);
  if (mac) {
    const arch = mac[1] === "aarch64" ? "arm64" : "x64";
    return {
      platform: "macos",
      kind: "dmg",
      arch,
      target: `${mac[1]}-apple-darwin`,
      label: arch === "arm64" ? "macOS DMG (Apple Silicon)" : "macOS DMG (Intel)",
    };
  }
  const macUpdater = name.match(
    /aka-desktop-.+-(aarch64|x86_64|universal)-apple-darwin\.app\.tar\.gz$/,
  );
  if (macUpdater) {
    const arch =
      macUpdater[1] === "aarch64"
        ? "arm64"
        : macUpdater[1] === "x86_64"
          ? "x64"
          : "universal";
    return {
      platform: "macos",
      kind: "updater",
      arch,
      target: `${macUpdater[1]}-apple-darwin`,
      updaterTarget:
        macUpdater[1] === "universal" ? "darwin-universal" : `darwin-${macUpdater[1]}`,
      updaterPriority: 10,
      label:
        arch === "arm64"
          ? "macOS Updater (Apple Silicon)"
          : arch === "x64"
            ? "macOS Updater (Intel)"
            : "macOS Updater (Universal)",
    };
  }
  if (/aka-desktop-.+-x86_64-pc-windows-msvc-setup\.exe$/.test(name)) {
    return {
      platform: "windows",
      kind: "exe",
      arch: "x64",
      target: "x86_64-pc-windows-msvc",
      updaterTarget: "windows-x86_64",
      updaterPriority: 10,
      label: "Windows Setup EXE",
    };
  }
  const linux = name.match(
    /aka-desktop-.+-(aarch64|x86_64)-unknown-linux-gnu\.(AppImage|deb|rpm)$/,
  );
  if (linux) {
    const arch = linux[1] === "aarch64" ? "arm64" : "x64";
    const kind = linux[2] === "AppImage" ? "appimage" : linux[2];
    return {
      platform: "linux",
      kind,
      arch,
      target: `${linux[1]}-unknown-linux-gnu`,
      label: `Linux ${linux[2]} (${arch})`,
    };
  }
  return null;
}

function groupedDownloads(assets) {
  const out = {};
  for (const asset of assets) {
    out[asset.platform] ??= {};
    const existing = out[asset.platform][asset.kind];
    if (!existing) {
      out[asset.platform][asset.kind] = asset;
    } else if (Array.isArray(existing)) {
      existing.push(asset);
    } else {
      out[asset.platform][asset.kind] = [existing, asset];
    }
  }
  return out;
}

function updaterPlatforms(assets) {
  const selected = new Map();
  for (const asset of assets) {
    if (!asset.updaterTarget || !asset.signature) continue;
    const existing = selected.get(asset.updaterTarget);
    if (!existing || (asset.updaterPriority ?? 100) < (existing.updaterPriority ?? 100)) {
      selected.set(asset.updaterTarget, asset);
    }
  }

  const out = {};
  for (const [target, asset] of [...selected.entries()].sort(([a], [b]) => a.localeCompare(b))) {
    out[target] = {
      signature: asset.signature,
      url: asset.url,
    };
  }
  return out;
}

function readSignature(dir, name, baseUrl) {
  for (const suffix of ["sig", "signature"]) {
    const signatureName = `${name}.${suffix}`;
    const signaturePath = path.join(dir, signatureName);
    if (!fs.existsSync(signaturePath) || !fs.statSync(signaturePath).isFile()) continue;
    const signature = fs.readFileSync(signaturePath, "utf8").trim();
    if (!signature) {
      console.warn(`warning: ignoring empty updater signature ${signaturePath}`);
      continue;
    }
    const signatureUrl = `${baseUrl}/${encodeURIComponent(signatureName)}`;
    return {
      signature,
      signatureName,
      signatureUrl,
    };
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
