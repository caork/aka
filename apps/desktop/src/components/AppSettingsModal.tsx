import { motion } from "framer-motion";
import { useState } from "react";
import {
  checkForAppUpdate,
  CURRENT_APP_VERSION,
  formatAssetSize,
  openExternalUrl,
  type ReleaseAsset,
  type ReleaseInfo,
} from "../release-api";
import { clearAppData } from "../repo-api";
import { useAppStore } from "../store";
import type { ThemeMode } from "../theme";
import Modal, { ErrorBar } from "./Modal";

const THEME_OPTIONS: { id: ThemeMode; label: string }[] = [
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
  { id: "auto", label: "Auto" },
];

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
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [openingUrl, setOpeningUrl] = useState<string | null>(null);
  const [release, setRelease] = useState<ReleaseInfo | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const checkUpdates = async () => {
    if (checkingUpdate) return;
    setCheckingUpdate(true);
    setNotice(null);
    setError(null);
    try {
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
        <div className="mb-3 flex items-start gap-4">
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-medium text-ink">Updates</div>
            <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
              hawkingrad release manifest
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
          <VersionTile label="Current" value={release?.currentVersion ?? CURRENT_APP_VERSION} />
          <VersionTile label="Latest" value={release?.latestVersion ?? "Not checked"} />
        </div>

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
                    disabled={!release.hasUpdate || openingUrl !== null}
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

function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}
