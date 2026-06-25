import { motion } from "framer-motion";
import { useEffect, useState } from "react";
import {
  checkForNativeAppUpdate,
  installNativeAppUpdate,
  NativeUpdaterUnavailableError,
  type InstallProgress,
  type NativeUpdateInfo,
} from "../app-update-api";
import { isDesktopRuntime } from "../desktop-api";
import {
  checkForAppUpdate,
  CURRENT_APP_VERSION,
  formatAssetSize,
  openExternalUrl,
  type ReleaseAsset,
  type ReleaseInfo,
} from "../release-api";
import { clearAppData, getAppSettings, setAppSettings } from "../repo-api";
import { useAppStore } from "../store";
import type { ThemeMode } from "../theme";
import Modal, { ErrorBar } from "./Modal";

const THEME_OPTIONS: { id: ThemeMode; label: string }[] = [
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
  { id: "auto", label: "Auto" },
];

const INDEX_MAX_DEFAULT = 60;
const INDEX_MAX_MIN = 10;
const INDEX_MAX_LIMIT = 24 * 60 * 60;
const LSP_MAX_DEFAULT = 30;
const LSP_MAX_MIN = 5;
const LSP_MAX_LIMIT = 10 * 60;

function clampIndexMaxSecs(value: number): number {
  if (!Number.isFinite(value)) return INDEX_MAX_DEFAULT;
  return Math.min(INDEX_MAX_LIMIT, Math.max(INDEX_MAX_MIN, Math.round(value)));
}

function clampLspMaxSecs(value: number): number {
  if (!Number.isFinite(value)) return LSP_MAX_DEFAULT;
  return Math.min(LSP_MAX_LIMIT, Math.max(LSP_MAX_MIN, Math.round(value)));
}

export default function AppSettingsModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose(): void;
}) {
  const themeMode = useAppStore((s) => s.themeMode);
  const setThemeMode = useAppStore((s) => s.setThemeMode);
  const resetRepos = useAppStore((s) => s.resetRepos);
  const [confirmClear, setConfirmClear] = useState(false);
  const [busy, setBusy] = useState(false);
  const [settingsBusy, setSettingsBusy] = useState(false);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [installingUpdate, setInstallingUpdate] = useState(false);
  const [openingUrl, setOpeningUrl] = useState<string | null>(null);
  const [nativeUpdate, setNativeUpdate] = useState<NativeUpdateInfo | null>(null);
  const [installProgress, setInstallProgress] = useState<InstallProgress | null>(null);
  const [release, setRelease] = useState<ReleaseInfo | null>(null);
  const [indexMaxSecs, setIndexMaxSecs] = useState(INDEX_MAX_DEFAULT);
  const [savedIndexMaxSecs, setSavedIndexMaxSecs] = useState(INDEX_MAX_DEFAULT);
  const [lspEnrichmentEnabled, setLspEnrichmentEnabled] = useState(false);
  const [savedLspEnrichmentEnabled, setSavedLspEnrichmentEnabled] = useState(false);
  const [lspEnrichmentMaxSecs, setLspEnrichmentMaxSecs] = useState(LSP_MAX_DEFAULT);
  const [savedLspEnrichmentMaxSecs, setSavedLspEnrichmentMaxSecs] =
    useState(LSP_MAX_DEFAULT);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void getAppSettings()
      .then((settings) => {
        if (cancelled) return;
        const next = clampIndexMaxSecs(settings.indexMaxSecs);
        setIndexMaxSecs(next);
        setSavedIndexMaxSecs(next);
        const nextLsp = clampLspMaxSecs(settings.lspEnrichmentMaxSecs);
        setLspEnrichmentEnabled(settings.lspEnrichmentEnabled);
        setSavedLspEnrichmentEnabled(settings.lspEnrichmentEnabled);
        setLspEnrichmentMaxSecs(nextLsp);
        setSavedLspEnrichmentMaxSecs(nextLsp);
      })
      .catch((e) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  const checkUpdates = async () => {
    if (checkingUpdate) return;
    setCheckingUpdate(true);
    setNotice(null);
    setError(null);
    setInstallProgress(null);
    setNativeUpdate(null);
    setRelease(null);
    try {
      if (isDesktopRuntime()) {
        try {
          const native = await checkForNativeAppUpdate();
          setNativeUpdate(native);
          setNotice(
            native.available
              ? `发现新版本 ${native.latestVersion}`
              : `已是最新版本 ${native.currentVersion}`,
          );
          return;
        } catch (e) {
          if (!(e instanceof NativeUpdaterUnavailableError)) {
            throw e;
          }
          setNotice("自动安装更新尚未配置，已切换到下载清单");
        }
      }

      const next = await checkForAppUpdate();
      setRelease(next);
      setNotice(
        next.hasUpdate
          ? `发现新版本 ${next.latestVersion}`
          : `已是最新版本 ${next.currentVersion}`,
      );
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCheckingUpdate(false);
    }
  };

  const installUpdate = async () => {
    if (installingUpdate || !nativeUpdate?.available) return;
    setInstallingUpdate(true);
    setNotice(null);
    setError(null);
    setInstallProgress({
      phase: "downloading",
      downloadedBytes: 0,
      totalBytes: undefined,
    });
    try {
      await installNativeAppUpdate(setInstallProgress);
      setNotice("更新已安装，正在重启");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setInstallingUpdate(false);
    }
  };

  const openReleaseUrl = async (url: string) => {
    if (openingUrl) return;
    setOpeningUrl(url);
    setError(null);
    try {
      await openExternalUrl(url);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setOpeningUrl(null);
    }
  };

  const clearData = async () => {
    if (!confirmClear) {
      setConfirmClear(true);
      setNotice(null);
      setError(null);
      return;
    }
    setBusy(true);
    setNotice(null);
    setError(null);
    try {
      await clearAppData();
      resetRepos();
      setConfirmClear(false);
      setNotice("本机数据已清理");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const saveIndexSettings = async () => {
    if (settingsBusy) return;
    setSettingsBusy(true);
    setNotice(null);
    setError(null);
    try {
      const settings = await setAppSettings({
        indexMaxSecs: clampIndexMaxSecs(indexMaxSecs),
        lspEnrichmentEnabled,
        lspEnrichmentMaxSecs: clampLspMaxSecs(lspEnrichmentMaxSecs),
      });
      const next = clampIndexMaxSecs(settings.indexMaxSecs);
      setIndexMaxSecs(next);
      setSavedIndexMaxSecs(next);
      const nextLsp = clampLspMaxSecs(settings.lspEnrichmentMaxSecs);
      setLspEnrichmentEnabled(settings.lspEnrichmentEnabled);
      setSavedLspEnrichmentEnabled(settings.lspEnrichmentEnabled);
      setLspEnrichmentMaxSecs(nextLsp);
      setSavedLspEnrichmentMaxSecs(nextLsp);
      setNotice("Indexing 设置已保存");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSettingsBusy(false);
    }
  };

  const indexDirty =
    clampIndexMaxSecs(indexMaxSecs) !== savedIndexMaxSecs ||
    lspEnrichmentEnabled !== savedLspEnrichmentEnabled ||
    clampLspMaxSecs(lspEnrichmentMaxSecs) !== savedLspEnrichmentMaxSecs;

  return (
    <Modal open={open} onClose={onClose} title="Settings" width={520}>
      {error && <ErrorBar message={error} />}
      {notice && (
        <div
          className="mb-3 rounded-[10px] px-3 py-2 text-[12px] text-ink-2"
          style={{
            background: "var(--success-fill)",
            boxShadow: "inset 0 0 0 0.5px rgba(52, 199, 89, 0.22)",
          }}
          data-testid="settings-notice"
        >
          {notice}
        </div>
      )}
      <div className="flex items-start gap-4">
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-medium text-ink">Appearance</div>
          <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
            选择 AKA 的显示模式
          </div>
        </div>
        <div
          className="segmented flex w-[176px] flex-none items-center gap-0.5 rounded-[10px] p-0.5"
          role="radiogroup"
          aria-label="Appearance"
          data-testid="theme-mode-switcher"
        >
          {THEME_OPTIONS.map((option) => {
            const active = option.id === themeMode;
            return (
              <button
                key={option.id}
                type="button"
                role="radio"
                aria-checked={active}
                onClick={() => setThemeMode(option.id)}
                className="focus-ring relative h-7 flex-1 rounded-[8px] text-[12px] font-medium transition-colors duration-150 ease-out"
                style={{ color: active ? "var(--ink)" : "var(--ink-2)" }}
                data-testid={`theme-mode-${option.id}`}
              >
                {active && (
                  <motion.span
                    layoutId="theme-mode-thumb"
                    transition={{ type: "spring", stiffness: 400, damping: 32 }}
                    className="segmented-thumb absolute inset-0 rounded-[8px]"
                  />
                )}
                <span className="relative z-10">{option.label}</span>
              </button>
            );
          })}
        </div>
      </div>

      <div className="themed-divider mt-5 border-t pt-4">
        <div className="flex items-start gap-4">
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-medium text-ink">Indexing timeout</div>
            <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
              全局索引时间预算；环境变量 AKA_INDEX_MAX_SECS 会临时覆盖此设置。
            </div>
          </div>
          <span className="cmd-input flex h-8 w-[96px] flex-none items-center px-2.5">
            <input
              type="number"
              min={INDEX_MAX_MIN}
              max={INDEX_MAX_LIMIT}
              step={10}
              value={indexMaxSecs}
              onChange={(e) => setIndexMaxSecs(Number(e.target.value))}
              onBlur={() => setIndexMaxSecs(clampIndexMaxSecs(indexMaxSecs))}
              disabled={settingsBusy}
              className="tabular h-full w-full text-[12.5px]"
              data-testid="index-max-secs-input"
            />
          </span>
        </div>
        <input
          type="range"
          min={INDEX_MAX_MIN}
          max={600}
          step={10}
          value={Math.min(600, clampIndexMaxSecs(indexMaxSecs))}
          onChange={(e) => setIndexMaxSecs(Number(e.target.value))}
          disabled={settingsBusy}
          className="mt-3 w-full"
          style={{ accentColor: "var(--accent)" }}
          aria-label="Indexing timeout seconds"
          data-testid="index-max-secs-slider"
        />
        <div className="mt-2 flex items-center gap-2 text-[11.5px] text-ink-3">
          <span>{formatDuration(clampIndexMaxSecs(indexMaxSecs))}</span>
          <button
            type="button"
            onClick={() => setIndexMaxSecs(INDEX_MAX_DEFAULT)}
            disabled={settingsBusy || indexMaxSecs === INDEX_MAX_DEFAULT}
            className="focus-ring rounded-[8px] px-2 py-1 transition-colors duration-150 ease-out hover:text-[var(--accent)] disabled:opacity-45"
          >
            Default
          </button>
          <button
            type="button"
            disabled={settingsBusy || !indexDirty}
            onClick={() => void saveIndexSettings()}
            className={`focus-ring ml-auto rounded-[9px] px-3 py-1.5 text-[12px] font-semibold transition-all duration-150 ease-out ${
              indexDirty ? "btn-primary" : "text-ink-3 opacity-60"
            }`}
            style={
              indexDirty
                ? undefined
                : { boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)" }
            }
            data-testid="index-max-secs-save"
          >
            {settingsBusy ? "Saving..." : "Save"}
          </button>
        </div>
        <div className="themed-divider mt-4 border-t pt-4">
          <div className="flex items-start gap-4">
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-medium text-ink">LSP enrichment</div>
              <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
                仅用于后续成熟语言服务增强；跳过或失败不影响 graph/search ready。
              </div>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={lspEnrichmentEnabled}
              onClick={() => setLspEnrichmentEnabled((v) => !v)}
              disabled={settingsBusy}
              className={`focus-ring relative h-7 w-12 flex-none rounded-full transition-colors duration-150 ease-out ${
                lspEnrichmentEnabled ? "bg-[var(--accent)]" : "bg-[var(--glass-strong)]"
              }`}
              style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)" }}
              data-testid="lsp-enrichment-switch"
            >
              <span
                className={`absolute top-1 h-5 w-5 rounded-full bg-white shadow-sm transition-transform duration-150 ease-out ${
                  lspEnrichmentEnabled ? "translate-x-6" : "translate-x-1"
                }`}
              />
            </button>
          </div>
          <div className="mt-3 flex items-center gap-3">
            <span className="cmd-input flex h-8 w-[96px] flex-none items-center px-2.5">
              <input
                type="number"
                min={LSP_MAX_MIN}
                max={LSP_MAX_LIMIT}
                step={5}
                value={lspEnrichmentMaxSecs}
                onChange={(e) => setLspEnrichmentMaxSecs(Number(e.target.value))}
                onBlur={() => setLspEnrichmentMaxSecs(clampLspMaxSecs(lspEnrichmentMaxSecs))}
                disabled={settingsBusy || !lspEnrichmentEnabled}
                className="tabular h-full w-full text-[12.5px]"
                data-testid="lsp-enrichment-max-secs-input"
              />
            </span>
            <span className="text-[11.5px] text-ink-3">
              {formatDuration(clampLspMaxSecs(lspEnrichmentMaxSecs))} max per optional pass
            </span>
          </div>
        </div>
      </div>

      <div className="themed-divider mt-5 border-t pt-4">
        <div className="mb-3 flex items-start gap-4">
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-medium text-ink">Updates</div>
            <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
              GitHub release manifest
            </div>
          </div>
          <button
            type="button"
            disabled={checkingUpdate}
            onClick={() => void checkUpdates()}
            className="focus-ring rounded-[10px] px-3 py-2 text-[12px] font-semibold transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-55"
            style={{
              color: "var(--accent-ink)",
              background: "var(--accent-fill)",
              boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)",
            }}
            data-testid="check-app-update"
          >
            {checkingUpdate ? "Checking..." : "Check for updates"}
          </button>
        </div>

        <div className="grid grid-cols-2 gap-2">
          <VersionTile
            label="Current"
            value={
              nativeUpdate?.currentVersion ??
              release?.currentVersion ??
              CURRENT_APP_VERSION
            }
          />
          <VersionTile
            label="Latest"
            value={nativeUpdate?.latestVersion ?? release?.latestVersion ?? "Not checked"}
          />
        </div>

        {nativeUpdate && (
          <div
            className="mt-3 rounded-[10px] p-3 text-[12px]"
            style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline)" }}
            data-testid="native-app-update-result"
          >
            <div className="mb-2 flex items-center gap-2">
              <span
                className="h-2 w-2 rounded-full"
                style={{
                  background: nativeUpdate.available
                    ? "var(--beacon)"
                    : "var(--success)",
                }}
              />
              <span className="font-medium text-ink">
                {nativeUpdate.available ? "Update ready to install" : "Up to date"}
              </span>
              {nativeUpdate.date && (
                <span className="ml-auto text-[11px] text-ink-3">
                  {formatDate(nativeUpdate.date)}
                </span>
              )}
            </div>

            {nativeUpdate.notes && (
              <div className="mb-2 line-clamp-3 text-[11.5px] leading-relaxed text-ink-3">
                {nativeUpdate.notes}
              </div>
            )}

            {nativeUpdate.available && (
              <>
                {installProgress && (
                  <UpdateProgressBar progress={installProgress} />
                )}
                <button
                  type="button"
                  disabled={installingUpdate}
                  onClick={() => void installUpdate()}
                  className="focus-ring mt-2 w-full rounded-[9px] px-3 py-1.5 text-[12px] font-semibold transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-55"
                  style={{
                    color: "var(--accent-ink)",
                    background: "var(--accent-fill)",
                    boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)",
                  }}
                  data-testid="install-app-update"
                >
                  {installingUpdate ? "Installing..." : "Install and restart"}
                </button>
              </>
            )}
          </div>
        )}

        {release && (
          <div
            className="mt-3 rounded-[10px] p-3 text-[12px]"
            style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline)" }}
            data-testid="app-update-result"
          >
            <div className="mb-2 flex items-center gap-2">
              <span
                className="h-2 w-2 rounded-full"
                style={{
                  background: release.hasUpdate ? "var(--beacon)" : "var(--success)",
                }}
              />
              <span className="font-medium text-ink">
                {release.hasUpdate ? "Update available" : "Up to date"}
              </span>
              {release.publishedAt && (
                <span className="ml-auto text-[11px] text-ink-3">
                  {formatDate(release.publishedAt)}
                </span>
              )}
            </div>

            {release.assets.length > 0 ? (
              <div className="space-y-2">
                {release.assets.map((asset) => (
                  <ReleaseAssetRow
                    key={asset.url}
                    asset={asset}
                    current={asset.platform === release.currentPlatform}
                    disabled={openingUrl !== null}
                    opening={openingUrl === asset.url}
                    onOpen={() => void openReleaseUrl(asset.url)}
                  />
                ))}
              </div>
            ) : (
              <div className="text-[11.5px] text-ink-3">
                No macOS DMG or Windows EXE package was listed in the manifest.
              </div>
            )}

            {release.hasUpdate && release.releaseUrl && (
              <button
                type="button"
                disabled={openingUrl !== null}
                onClick={() => void openReleaseUrl(release.releaseUrl!)}
                className="focus-ring mt-2 w-full rounded-[9px] px-3 py-1.5 text-[12px] font-semibold text-ink-2 transition-colors duration-150 ease-out hover:text-ink disabled:cursor-not-allowed disabled:opacity-55"
                style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline)" }}
                data-testid="open-app-release"
              >
                {openingUrl === release.releaseUrl ? "Opening..." : "Open release page"}
              </button>
            )}
          </div>
        )}
      </div>

      <div className="themed-divider mt-5 border-t pt-4">
        <div className="mb-3">
          <div className="text-[13px] font-medium text-ink">Local data</div>
          <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
            清理桌面版的仓库注册表、索引、checkout 和导入记录。
          </div>
        </div>
        <button
          type="button"
          disabled={busy}
          onClick={clearData}
          className="focus-ring w-full rounded-[10px] px-3 py-2 text-[12.5px] font-semibold transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-55"
          style={{
            color: "var(--danger-ink)",
            background: confirmClear ? "var(--danger-fill)" : "var(--subtle-fill)",
            boxShadow: confirmClear
              ? "inset 0 0 0 0.5px var(--danger-border)"
              : "inset 0 0 0 0.5px var(--hairline)",
          }}
          data-testid="clear-app-data"
        >
          {busy
            ? "Clearing..."
            : confirmClear
              ? "Confirm clear local data"
              : "Clear local data"}
        </button>
      </div>
    </Modal>
  );
}

function VersionTile({ label, value }: { label: string; value: string }) {
  return (
    <div
      className="rounded-[10px] px-3 py-2"
      style={{ background: "var(--subtle-fill)" }}
    >
      <div className="text-[10.5px] font-semibold uppercase text-ink-3">{label}</div>
      <div className="mt-0.5 truncate text-[13px] font-medium text-ink">{value}</div>
    </div>
  );
}

function ReleaseAssetRow({
  asset,
  current,
  disabled,
  opening,
  onOpen,
}: {
  asset: ReleaseAsset;
  current: boolean;
  disabled: boolean;
  opening: boolean;
  onOpen(): void;
}) {
  const size = formatAssetSize(asset.size);
  return (
    <div
      className="flex items-center gap-3 rounded-[9px] px-2.5 py-2"
      style={{ background: "var(--subtle-fill-2)" }}
      data-testid={`release-asset-${asset.platform}-${asset.kind}`}
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="text-[12px] font-medium text-ink">{asset.label}</span>
          {current && (
            <span
              className="rounded-[6px] px-1.5 py-0.5 text-[10px] font-semibold text-ink-2"
              style={{ background: "var(--subtle-fill)" }}
            >
              this device
            </span>
          )}
        </div>
        <div className="mt-0.5 truncate text-[11px] text-ink-3" title={asset.name}>
          {asset.name}
          {size ? ` · ${size}` : ""}
        </div>
      </div>
      <button
        type="button"
        disabled={disabled}
        onClick={onOpen}
        className="focus-ring rounded-[8px] px-2.5 py-1.5 text-[11.5px] font-semibold transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-45"
        style={{
          color: "var(--accent-ink)",
          boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)",
        }}
      >
        {opening ? "Opening..." : "Download"}
      </button>
    </div>
  );
}

function UpdateProgressBar({ progress }: { progress: InstallProgress }) {
  const percent =
    progress.totalBytes && progress.totalBytes > 0
      ? Math.min(100, Math.round((progress.downloadedBytes / progress.totalBytes) * 100))
      : null;
  const label =
    progress.phase === "installing"
      ? "Installing"
      : progress.phase === "installed"
        ? "Installed"
        : percent === null
          ? "Downloading"
          : `Downloading ${percent}%`;
  return (
    <div className="mt-2">
      <div className="mb-1 flex items-center justify-between text-[11px] text-ink-3">
        <span>{label}</span>
        {percent !== null && <span>{formatBytes(progress.downloadedBytes)}</span>}
      </div>
      <div
        className="h-1.5 overflow-hidden rounded-full"
        style={{ background: "var(--subtle-fill)" }}
      >
        <div
          className="h-full rounded-full transition-[width] duration-150 ease-out"
          style={{
            width: `${percent ?? 18}%`,
            background: "var(--accent)",
          }}
        />
      </div>
    </div>
  );
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 KB";
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function formatDuration(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return rest === 0 ? `${minutes}m` : `${minutes}m ${rest}s`;
}

function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}
