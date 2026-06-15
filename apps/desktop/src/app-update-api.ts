import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent } from "@tauri-apps/plugin-updater";
import { markClientIntegrationSyncAfterRestart } from "./client-integration-api";
import { isDesktopRuntime } from "./desktop-api";
import { CURRENT_APP_VERSION } from "./release-api";

export type NativeUpdatePhase =
  | "checking"
  | "available"
  | "downloading"
  | "installing"
  | "installed"
  | "idle";

export interface NativeUpdateInfo {
  available: boolean;
  currentVersion: string;
  latestVersion: string;
  date?: string;
  notes?: string;
  rawJson: Record<string, unknown>;
}

export interface InstallProgress {
  phase: NativeUpdatePhase;
  downloadedBytes: number;
  totalBytes?: number;
}

export class NativeUpdaterUnavailableError extends Error {
  constructor(message = "原生自动更新未配置") {
    super(message);
    this.name = "NativeUpdaterUnavailableError";
  }
}

type NativeUpdateHandle = Awaited<ReturnType<typeof check>>;

let pendingUpdate: NativeUpdateHandle = null;

export async function checkForNativeAppUpdate(): Promise<NativeUpdateInfo> {
  if (!isDesktopRuntime()) {
    throw new NativeUpdaterUnavailableError("浏览器预览环境不支持自动安装更新");
  }

  let update: NativeUpdateHandle;
  try {
    update = await check({ timeout: 10_000 });
  } catch (e) {
    throw normalizeNativeUpdaterError(e);
  }

  pendingUpdate = update;
  if (!update) {
    return {
      available: false,
      currentVersion: CURRENT_APP_VERSION,
      latestVersion: CURRENT_APP_VERSION,
      rawJson: {},
    };
  }

  return {
    available: true,
    currentVersion: update.currentVersion,
    latestVersion: update.version,
    date: update.date,
    notes: update.body,
    rawJson: update.rawJson,
  };
}

export async function installNativeAppUpdate(
  onProgress: (progress: InstallProgress) => void,
): Promise<void> {
  if (!pendingUpdate) {
    const info = await checkForNativeAppUpdate();
    if (!info.available || !pendingUpdate) {
      throw new Error("没有可安装的更新");
    }
  }

  let downloadedBytes = 0;
  let totalBytes: number | undefined;
  try {
    onProgress({ phase: "downloading", downloadedBytes, totalBytes });
    await pendingUpdate.downloadAndInstall((event: DownloadEvent) => {
      if (event.event === "Started") {
        downloadedBytes = 0;
        totalBytes = event.data.contentLength;
        onProgress({ phase: "downloading", downloadedBytes, totalBytes });
      } else if (event.event === "Progress") {
        downloadedBytes += event.data.chunkLength;
        onProgress({ phase: "downloading", downloadedBytes, totalBytes });
      } else if (event.event === "Finished") {
        onProgress({ phase: "installing", downloadedBytes, totalBytes });
      }
    });
    onProgress({ phase: "installed", downloadedBytes, totalBytes });
    markClientIntegrationSyncAfterRestart();
    await relaunch();
  } catch (e) {
    throw normalizeNativeUpdaterError(e);
  } finally {
    await pendingUpdate?.close().catch(() => undefined);
    pendingUpdate = null;
  }
}

function normalizeNativeUpdaterError(e: unknown): Error {
  const message = e instanceof Error ? e.message : String(e);
  if (
    /updater.*not configured|No updater endpoints|does not have any endpoints set|public key|pubkey|signature/i.test(
      message,
    )
  ) {
    return new NativeUpdaterUnavailableError(message);
  }
  return e instanceof Error ? e : new Error(message);
}
