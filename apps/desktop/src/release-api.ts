import desktopPackage from "../package.json";
import { asDesktopError, invokeDesktop, isDesktopRuntime } from "./desktop-api";

export const RELEASE_MANIFEST_URL =
  "https://aka.hawkingrad.com/releases/latest.json";

export const CURRENT_APP_VERSION =
  typeof desktopPackage.version === "string" ? desktopPackage.version : "0.0.0";

type JsonObject = Record<string, unknown>;

export type ReleasePlatform = "macos" | "windows";
export type ReleaseAssetKind = "dmg" | "exe";

export interface ReleaseAsset {
  platform: ReleasePlatform;
  kind: ReleaseAssetKind;
  label: string;
  url: string;
  name: string;
  size?: number;
  sha256?: string;
}

export interface ReleaseInfo {
  currentVersion: string;
  latestVersion: string;
  hasUpdate: boolean;
  manifestUrl: string;
  releaseUrl?: string;
  notes?: string;
  publishedAt?: string;
  fetchedAt: string;
  assets: ReleaseAsset[];
  currentPlatform: ReleasePlatform | "linux" | "unknown";
}

export async function checkForAppUpdate(
  manifestUrl = RELEASE_MANIFEST_URL,
): Promise<ReleaseInfo> {
  const [currentVersion, manifest] = await Promise.all([
    getCurrentAppVersion(),
    fetchReleaseManifest(manifestUrl),
  ]);
  const latestVersion = readVersion(manifest);
  if (!latestVersion) {
    throw new Error("发布清单缺少 version/latestVersion 字段");
  }

  const currentPlatform = currentReleasePlatform();
  const assets = sortAssetsForPlatform(collectAssets(manifest), currentPlatform);
  return {
    currentVersion,
    latestVersion,
    hasUpdate: compareVersions(latestVersion, currentVersion) > 0,
    manifestUrl,
    releaseUrl: readFirstString(manifest, [
      "releaseUrl",
      "release_url",
      "html_url",
      "url",
    ]),
    notes: readFirstString(manifest, ["notes", "body", "changelog"]),
    publishedAt: readFirstString(manifest, [
      "publishedAt",
      "published_at",
      "pub_date",
      "date",
    ]),
    fetchedAt: new Date().toISOString(),
    assets,
    currentPlatform,
  };
}

export async function openExternalUrl(url: string): Promise<void> {
  if (isDesktopRuntime()) {
    try {
      await invokeDesktop("open_url", { url });
      return;
    } catch (e) {
      throw asDesktopError(e, "打开链接失败");
    }
  }

  const opened = window.open(url, "_blank", "noopener,noreferrer");
  if (!opened) {
    window.location.assign(url);
  }
}

export function formatAssetSize(size: number | undefined): string | null {
  if (!Number.isFinite(size) || !size) return null;
  if (size < 1024 * 1024) return `${Math.round(size / 1024)} KB`;
  return `${(size / 1024 / 1024).toFixed(size < 100 * 1024 * 1024 ? 1 : 0)} MB`;
}

async function getCurrentAppVersion(): Promise<string> {
  if (!isDesktopRuntime()) return CURRENT_APP_VERSION;
  try {
    const version = await invokeDesktop<string>("app_version");
    return version || CURRENT_APP_VERSION;
  } catch {
    return CURRENT_APP_VERSION;
  }
}

async function fetchReleaseManifest(url: string): Promise<JsonObject> {
  const text =
    typeof window.fetch === "function"
      ? await fetchReleaseManifestWithFetch(url)
      : await fetchReleaseManifestWithXhr(url);

  try {
    const json = JSON.parse(text) as unknown;
    if (json && typeof json === "object" && !Array.isArray(json)) {
      return json as JsonObject;
    }
  } catch {
    /* handled below with a clearer message */
  }
  throw new Error("发布源未返回 JSON，可能尚未公开或被登录网关保护");
}

async function fetchReleaseManifestWithFetch(url: string): Promise<string> {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), 10_000);
  let response: Response;
  try {
    response = await window.fetch(url, {
      headers: { accept: "application/json" },
      cache: "no-store",
      signal: controller.signal,
    });
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") {
      throw new Error("检查更新超时");
    }
    throw new Error("无法连接发布源");
  } finally {
    window.clearTimeout(timeout);
  }

  if (!response.ok) {
    throw new Error(`发布源请求失败（HTTP ${response.status}）`);
  }

  return response.text();
}

function fetchReleaseManifestWithXhr(url: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("GET", url, true);
    xhr.timeout = 10_000;
    xhr.setRequestHeader("accept", "application/json");

    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        resolve(xhr.responseText);
        return;
      }
      reject(new Error(`发布源请求失败（HTTP ${xhr.status}）`));
    };
    xhr.onerror = () => reject(new Error("无法连接发布源"));
    xhr.ontimeout = () => reject(new Error("检查更新超时"));
    xhr.send();
  });
}

function readVersion(manifest: JsonObject): string | null {
  const value = readFirstString(manifest, [
    "version",
    "latestVersion",
    "latest_version",
    "tag",
    "tag_name",
    "name",
  ]);
  return value ? cleanVersion(value) : null;
}

function cleanVersion(version: string): string {
  return version.trim().replace(/^aka-desktop-/i, "").replace(/^v/i, "");
}

function readFirstString(object: JsonObject, keys: string[]): string | undefined {
  for (const key of keys) {
    const value = object[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return undefined;
}

function collectAssets(manifest: JsonObject): ReleaseAsset[] {
  const assets: ReleaseAsset[] = [];
  const seen = new Set<string>();

  const add = (asset: ReleaseAsset | null) => {
    if (!asset || seen.has(asset.url)) return;
    seen.add(asset.url);
    assets.push(asset);
  };

  for (const key of ["assets", "files", "downloads", "packages"]) {
    const value = manifest[key];
    if (Array.isArray(value)) {
      for (const item of value) add(assetFromUnknown(item));
    } else if (value && typeof value === "object") {
      collectFromObject(value as JsonObject, add);
    }
  }

  const platforms = manifest.platforms;
  if (platforms && typeof platforms === "object") {
    collectFromObject(platforms as JsonObject, add);
  }

  for (const platform of ["macos", "darwin", "windows", "win"] as const) {
    const value = manifest[platform];
    if (value && typeof value === "object") {
      collectFromObject(value as JsonObject, add, platform);
    }
  }

  return assets.sort((a, b) => {
    const platformOrder = a.platform.localeCompare(b.platform);
    if (platformOrder !== 0) return platformOrder;
    return a.kind.localeCompare(b.kind);
  });
}

function collectFromObject(
  object: JsonObject,
  add: (asset: ReleaseAsset | null) => void,
  platformHint?: string,
) {
  for (const [key, value] of Object.entries(object)) {
    if (typeof value === "string") {
      add(assetFromUnknown(value, platformHint ?? key, key));
    } else if (value && typeof value === "object") {
      const nested = value as JsonObject;
      add(assetFromUnknown(nested, platformHint ?? key, key));
      if (!readUrl(nested)) collectFromObject(nested, add, platformHint ?? key);
    }
  }
}

function assetFromUnknown(
  raw: unknown,
  platformHint?: string,
  kindHint?: string,
): ReleaseAsset | null {
  const object =
    raw && typeof raw === "object" && !Array.isArray(raw)
      ? (raw as JsonObject)
      : null;
  const url = typeof raw === "string" ? raw : object ? readUrl(object) : undefined;
  if (!url || !/^https?:\/\//i.test(url)) return null;

  const name = object ? readAssetName(object, url) : nameFromUrl(url);
  const searchText = [
    platformHint,
    kindHint,
    url,
    name,
    object ? readFirstString(object, ["platform", "os", "target", "arch"]) : "",
    object ? readFirstString(object, ["kind", "type", "format", "ext"]) : "",
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  const platform = inferPlatform(searchText);
  const kind = inferKind(searchText);
  if (!platform || !kind) return null;
  const label = object ? readFirstString(object, ["label", "title"]) : undefined;

  return {
    platform,
    kind,
    label: label ?? (platform === "macos" ? "macOS DMG" : "Windows EXE"),
    url,
    name,
    size: object ? readNumber(object, ["size", "sizeBytes", "size_bytes"]) : undefined,
    sha256: object ? readFirstString(object, ["sha256", "checksum"]) : undefined,
  };
}

function readUrl(object: JsonObject): string | undefined {
  return readFirstString(object, [
    "url",
    "downloadUrl",
    "download_url",
    "browserDownloadUrl",
    "browser_download_url",
    "href",
  ]);
}

function readAssetName(object: JsonObject, url: string): string {
  return (
    readFirstString(object, ["name", "fileName", "filename", "label"]) ??
    nameFromUrl(url)
  );
}

function nameFromUrl(url: string): string {
  try {
    const pathname = new URL(url).pathname;
    const last = pathname.split("/").filter(Boolean).pop();
    return last ? decodeURIComponent(last) : url;
  } catch {
    return url;
  }
}

function readNumber(object: JsonObject, keys: string[]): number | undefined {
  for (const key of keys) {
    const value = object[key];
    if (typeof value === "number" && Number.isFinite(value)) return value;
    if (typeof value === "string" && value.trim()) {
      const parsed = Number(value);
      if (Number.isFinite(parsed)) return parsed;
    }
  }
  return undefined;
}

function inferPlatform(text: string): ReleasePlatform | null {
  if (/(macos|darwin|apple|\.dmg\b)/i.test(text)) return "macos";
  if (/(windows|\bwin\b|win32|win64|msvc|setup\.exe\b|\.exe\b)/i.test(text)) {
    return "windows";
  }
  return null;
}

function inferKind(text: string): ReleaseAssetKind | null {
  if (/(\bdmg\b|\.dmg\b)/i.test(text)) return "dmg";
  if (/(\bexe\b|setup\.exe\b|\.exe\b)/i.test(text)) return "exe";
  return null;
}

function currentReleasePlatform(): ReleaseInfo["currentPlatform"] {
  const platform = navigator.platform.toLowerCase();
  const userAgent = navigator.userAgent.toLowerCase();
  if (platform.includes("mac") || userAgent.includes("mac os")) return "macos";
  if (platform.includes("win") || userAgent.includes("windows")) return "windows";
  if (platform.includes("linux") || userAgent.includes("linux")) return "linux";
  return "unknown";
}

function sortAssetsForPlatform(
  assets: ReleaseAsset[],
  currentPlatform: ReleaseInfo["currentPlatform"],
): ReleaseAsset[] {
  return [...assets].sort((a, b) => {
    const aCurrent = a.platform === currentPlatform ? 0 : 1;
    const bCurrent = b.platform === currentPlatform ? 0 : 1;
    if (aCurrent !== bCurrent) return aCurrent - bCurrent;
    const platformOrder = a.platform.localeCompare(b.platform);
    if (platformOrder !== 0) return platformOrder;
    return a.kind.localeCompare(b.kind);
  });
}

function compareVersions(a: string, b: string): number {
  const left = parseVersion(a);
  const right = parseVersion(b);
  const width = Math.max(left.parts.length, right.parts.length);
  for (let i = 0; i < width; i += 1) {
    const l = left.parts[i] ?? 0;
    const r = right.parts[i] ?? 0;
    if (l !== r) return l > r ? 1 : -1;
  }
  if (left.pre === right.pre) return 0;
  if (!left.pre) return 1;
  if (!right.pre) return -1;
  return left.pre.localeCompare(right.pre);
}

function parseVersion(version: string): { parts: number[]; pre: string } {
  const [main, pre = ""] = cleanVersion(version).split("-", 2);
  return {
    parts: main.split(".").map((part) => {
      const parsed = Number.parseInt(part.replace(/\D.*/, ""), 10);
      return Number.isFinite(parsed) ? parsed : 0;
    }),
    pre,
  };
}
