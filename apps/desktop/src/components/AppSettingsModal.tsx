import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  checkForNativeAppUpdate,
  installNativeAppUpdate,
  NativeUpdaterUnavailableError,
  type InstallProgress,
  type NativeUpdateInfo,
} from "../app-update-api";
import {
  getClientIntegrationsStatus,
  installClientIntegration,
  type ClientIntegrationId,
  type ClientIntegrationsStatus,
  type ClientIntegrationStatus,
} from "../client-integrations-api";
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
import { ErrorBar } from "./Modal";
import { RepoSettingsPanel } from "./RepoSettingsModal";

const THEME_OPTIONS: { id: ThemeMode; label: string }[] = [
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
  { id: "auto", label: "Auto" },
];

const INDEX_MAX_DEFAULT = 60;
const INDEX_MAX_MIN = 10;
const INDEX_MAX_LIMIT = 24 * 60 * 60;
const OSS_ANALYZER_MAX_DEFAULT = 30;
const OSS_ANALYZER_MAX_MIN = 5;
const OSS_ANALYZER_MAX_LIMIT = 10 * 60;

const OSS_ANALYZERS = [
  { name: "SCIP", detail: "Sourcegraph protocol · Apache-2.0" },
  { name: "Stack graphs", detail: "tree-sitter · MIT/Apache-2.0" },
  { name: "Pyright", detail: "Python · MIT" },
  { name: "gopls", detail: "Go · BSD-3-Clause" },
  { name: "TypeScript LS", detail: "TS/JS · MIT/Apache-2.0" },
  { name: "rust-analyzer", detail: "Rust · MIT/Apache-2.0" },
  { name: "JDT LS", detail: "Java · EPL-2.0" },
];

const SETTINGS_NAV = [
  { id: "appearance", label: "Appearance", detail: "Display" },
  { id: "repos", label: "Repositories", detail: "Per repo" },
  { id: "indexing", label: "Indexing", detail: "Timeouts" },
  { id: "updates", label: "Updates", detail: "Release" },
  { id: "plugins", label: "Agent plugins", detail: "Clients" },
  { id: "data", label: "Local data", detail: "Reset" },
] as const;

type SettingsSectionId = (typeof SETTINGS_NAV)[number]["id"];

function clampIndexMaxSecs(value: number): number {
  if (!Number.isFinite(value)) return INDEX_MAX_DEFAULT;
  return Math.min(INDEX_MAX_LIMIT, Math.max(INDEX_MAX_MIN, Math.round(value)));
}

function clampOssAnalyzerMaxSecs(value: number): number {
  if (!Number.isFinite(value)) return OSS_ANALYZER_MAX_DEFAULT;
  return Math.min(
    OSS_ANALYZER_MAX_LIMIT,
    Math.max(OSS_ANALYZER_MAX_MIN, Math.round(value)),
  );
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
  const repos = useAppStore((s) => s.repos);
  const selectedRepoId = useAppStore((s) => s.selectedRepoId);
  const selectRepo = useAppStore((s) => s.selectRepo);
  const resetRepos = useAppStore((s) => s.resetRepos);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const sectionRefs = useRef<Record<SettingsSectionId, HTMLElement | null>>({
    appearance: null,
    repos: null,
    indexing: null,
    updates: null,
    plugins: null,
    data: null,
  });
  const [confirmClear, setConfirmClear] = useState(false);
  const [busy, setBusy] = useState(false);
  const [settingsBusy, setSettingsBusy] = useState(false);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [installingUpdate, setInstallingUpdate] = useState(false);
  const [clientIntegrations, setClientIntegrations] =
    useState<ClientIntegrationsStatus | null>(null);
  const [clientIntegrationsBusy, setClientIntegrationsBusy] = useState(false);
  const [clientIntegrationAction, setClientIntegrationAction] = useState<
    string | null
  >(null);
  const [openingUrl, setOpeningUrl] = useState<string | null>(null);
  const [nativeUpdate, setNativeUpdate] = useState<NativeUpdateInfo | null>(
    null,
  );
  const [installProgress, setInstallProgress] =
    useState<InstallProgress | null>(null);
  const [release, setRelease] = useState<ReleaseInfo | null>(null);
  const [indexMaxSecs, setIndexMaxSecs] = useState(INDEX_MAX_DEFAULT);
  const [savedIndexMaxSecs, setSavedIndexMaxSecs] = useState(INDEX_MAX_DEFAULT);
  const [ossAnalyzerEnrichmentEnabled, setOssAnalyzerEnrichmentEnabled] =
    useState(false);
  const [
    savedOssAnalyzerEnrichmentEnabled,
    setSavedOssAnalyzerEnrichmentEnabled,
  ] = useState(false);
  const [ossAnalyzerEnrichmentMaxSecs, setOssAnalyzerEnrichmentMaxSecs] =
    useState(OSS_ANALYZER_MAX_DEFAULT);
  const [
    savedOssAnalyzerEnrichmentMaxSecs,
    setSavedOssAnalyzerEnrichmentMaxSecs,
  ] = useState(OSS_ANALYZER_MAX_DEFAULT);
  const [scipIndexPath, setScipIndexPath] = useState("");
  const [savedScipIndexPath, setSavedScipIndexPath] = useState("");
  const [ossAnalyzerFactsPath, setOssAnalyzerFactsPath] = useState("");
  const [savedOssAnalyzerFactsPath, setSavedOssAnalyzerFactsPath] =
    useState("");
  const [activeSection, setActiveSection] =
    useState<SettingsSectionId>("appearance");
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const repoSettingsRepo =
    repos.find((repo) => repo.id === selectedRepoId) ?? repos[0] ?? null;

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void getAppSettings()
      .then((settings) => {
        if (cancelled) return;
        const next = clampIndexMaxSecs(settings.indexMaxSecs);
        setIndexMaxSecs(next);
        setSavedIndexMaxSecs(next);
        const nextOss = clampOssAnalyzerMaxSecs(
          settings.ossAnalyzerEnrichmentMaxSecs,
        );
        setOssAnalyzerEnrichmentEnabled(settings.ossAnalyzerEnrichmentEnabled);
        setSavedOssAnalyzerEnrichmentEnabled(
          settings.ossAnalyzerEnrichmentEnabled,
        );
        setOssAnalyzerEnrichmentMaxSecs(nextOss);
        setSavedOssAnalyzerEnrichmentMaxSecs(nextOss);
        const nextScipPath = settings.scipIndexPath ?? "";
        setScipIndexPath(nextScipPath);
        setSavedScipIndexPath(nextScipPath);
        const nextFactsPath = settings.ossAnalyzerFactsPath ?? "";
        setOssAnalyzerFactsPath(nextFactsPath);
        setSavedOssAnalyzerFactsPath(nextFactsPath);
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

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setClientIntegrationsBusy(true);
    void getClientIntegrationsStatus()
      .then((status) => {
        if (!cancelled) {
          setClientIntegrations(status);
        }
      })
      .catch((e) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setClientIntegrationsBusy(false);
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

  const runClientIntegration = async (
    client: ClientIntegrationId,
    reinstall: boolean,
  ) => {
    const actionKey = `${client}:${reinstall ? "reinstall" : "install"}`;
    if (clientIntegrationAction) return;
    setClientIntegrationAction(actionKey);
    setNotice(null);
    setError(null);
    try {
      const next = await installClientIntegration({ client, reinstall });
      setClientIntegrations(next);
      setNotice(next.lastAction ?? "客户端插件已更新");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setClientIntegrationAction(null);
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
        ossAnalyzerEnrichmentEnabled,
        ossAnalyzerEnrichmentMaxSecs: clampOssAnalyzerMaxSecs(
          ossAnalyzerEnrichmentMaxSecs,
        ),
        scipIndexPath: scipIndexPath.trim() ? scipIndexPath.trim() : null,
        ossAnalyzerFactsPath: ossAnalyzerFactsPath.trim()
          ? ossAnalyzerFactsPath.trim()
          : null,
        lspEnrichmentEnabled: ossAnalyzerEnrichmentEnabled,
        lspEnrichmentMaxSecs: clampOssAnalyzerMaxSecs(
          ossAnalyzerEnrichmentMaxSecs,
        ),
      });
      const next = clampIndexMaxSecs(settings.indexMaxSecs);
      setIndexMaxSecs(next);
      setSavedIndexMaxSecs(next);
      const nextOss = clampOssAnalyzerMaxSecs(
        settings.ossAnalyzerEnrichmentMaxSecs,
      );
      setOssAnalyzerEnrichmentEnabled(settings.ossAnalyzerEnrichmentEnabled);
      setSavedOssAnalyzerEnrichmentEnabled(
        settings.ossAnalyzerEnrichmentEnabled,
      );
      setOssAnalyzerEnrichmentMaxSecs(nextOss);
      setSavedOssAnalyzerEnrichmentMaxSecs(nextOss);
      const nextScipPath = settings.scipIndexPath ?? "";
      setScipIndexPath(nextScipPath);
      setSavedScipIndexPath(nextScipPath);
      const nextFactsPath = settings.ossAnalyzerFactsPath ?? "";
      setOssAnalyzerFactsPath(nextFactsPath);
      setSavedOssAnalyzerFactsPath(nextFactsPath);
      setNotice("Indexing 设置已保存");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSettingsBusy(false);
    }
  };

  const indexDirty =
    clampIndexMaxSecs(indexMaxSecs) !== savedIndexMaxSecs ||
    ossAnalyzerEnrichmentEnabled !== savedOssAnalyzerEnrichmentEnabled ||
    clampOssAnalyzerMaxSecs(ossAnalyzerEnrichmentMaxSecs) !==
      savedOssAnalyzerEnrichmentMaxSecs ||
    scipIndexPath.trim() !== savedScipIndexPath ||
    ossAnalyzerFactsPath.trim() !== savedOssAnalyzerFactsPath;

  const setSectionRef =
    (id: SettingsSectionId) => (node: HTMLElement | null) => {
      sectionRefs.current[id] = node;
    };

  const scrollToSection = (id: SettingsSectionId) => {
    setActiveSection(id);
    sectionRefs.current[id]?.scrollIntoView({
      behavior: "smooth",
      block: "start",
    });
  };

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => {
      contentRef.current?.scrollTo({ top: 0 });
      setActiveSection("appearance");
    });
    return () => window.cancelAnimationFrame(frame);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const content = contentRef.current;
    if (!content) return;

    const updateActiveSection = () => {
      const topEdge = content.getBoundingClientRect().top + 32;
      let next: SettingsSectionId = "appearance";
      for (const item of SETTINGS_NAV) {
        const section = sectionRefs.current[item.id];
        if (section && section.getBoundingClientRect().top <= topEdge) {
          next = item.id;
        }
      }
      setActiveSection(next);
    };

    updateActiveSection();
    content.addEventListener("scroll", updateActiveSection, { passive: true });
    window.addEventListener("resize", updateActiveSection);
    return () => {
      content.removeEventListener("scroll", updateActiveSection);
      window.removeEventListener("resize", updateActiveSection);
    };
  }, [open]);

  const sectionPanelStyle = {
    background: "var(--popover-bg)",
    boxShadow:
      "inset 0 0 0 0.5px var(--hairline), 0 1px 2px rgba(16, 24, 40, 0.035)",
  };

  const navigation = (
    <nav
      className="rounded-[14px] p-1"
      style={{
        background: "var(--subtle-fill)",
        boxShadow: "inset 0 0 0 0.5px var(--hairline)",
      }}
      aria-label="Settings sections"
    >
      {SETTINGS_NAV.map((item) => {
        const active = activeSection === item.id;
        return (
          <button
            key={item.id}
            type="button"
            onClick={() => scrollToSection(item.id)}
            className="focus-ring relative flex w-full min-w-0 items-center gap-2 rounded-[10px] px-3 py-2.5 text-left transition-colors duration-150 ease-out"
            style={{
              color: active ? "var(--ink)" : "var(--ink-2)",
              background: active ? "var(--popover-bg)" : "transparent",
              boxShadow: active ? "var(--shadow-float)" : "none",
            }}
          >
            <span
              className="h-2 w-2 flex-none rounded-full"
              style={{
                background: active ? "var(--accent)" : "var(--hairline-strong)",
              }}
            />
            <span className="min-w-0 flex-1">
              <span className="block truncate text-[12.5px] font-semibold">
                {item.label}
              </span>
              <span className="block truncate text-[10.5px] text-ink-3">
                {item.detail}
              </span>
            </span>
          </button>
        );
      })}
    </nav>
  );

  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.15, ease: "easeOut" }}
          className="fixed inset-0 z-50"
          style={{
            background: "var(--popover-bg)",
          }}
          data-testid="app-settings-fullscreen"
        >
          <motion.div
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 8 }}
            transition={{ type: "spring", stiffness: 300, damping: 30 }}
            className="flex h-full min-h-0 overflow-hidden"
            style={{ background: "var(--popover-bg)" }}
            role="dialog"
            aria-modal="true"
            aria-label="Settings"
          >
            <aside className="hidden w-[244px] flex-none flex-col border-r border-[var(--main-divider)] bg-[var(--popover-bg)] px-4 py-4 lg:flex">
              <button
                type="button"
                onClick={onClose}
                className="focus-ring flex h-9 w-9 items-center justify-center rounded-[10px] text-[20px] leading-none text-ink-2 transition-colors duration-150 ease-out hover:bg-[var(--hover-fill)] hover:text-ink"
                aria-label="Close settings"
                data-testid="close-app-settings"
              >
                ←
              </button>
              <div className="mt-5 px-1">
                <div className="text-[18px] font-semibold tracking-tight text-ink">
                  Settings
                </div>
                <div className="mt-1 text-[11.5px] leading-relaxed text-ink-3">
                  AKA desktop
                </div>
              </div>
              <div className="mt-5">{navigation}</div>
            </aside>

            <div className="flex min-w-0 flex-1 flex-col bg-[var(--popover-bg)]">
              <header className="flex flex-none items-center gap-3 border-b border-[var(--main-divider)] px-4 py-3 lg:hidden">
                <button
                  type="button"
                  onClick={onClose}
                  className="focus-ring flex h-9 w-9 flex-none items-center justify-center rounded-[10px] text-[20px] leading-none text-ink-2 transition-colors duration-150 ease-out hover:bg-[var(--hover-fill)] hover:text-ink"
                  aria-label="Close settings"
                  data-testid="close-app-settings-mobile"
                >
                  ←
                </button>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[16px] font-semibold tracking-tight text-ink">
                    Settings
                  </div>
                  <div className="truncate text-[11px] text-ink-3">
                    AKA desktop
                  </div>
                </div>
              </header>
              <div className="border-b border-[var(--main-divider)] px-4 py-2 lg:hidden">
                <div className="scroll-area flex gap-2 overflow-x-auto pb-1">
                  {SETTINGS_NAV.map((item) => {
                    const active = activeSection === item.id;
                    return (
                      <button
                        key={item.id}
                        type="button"
                        onClick={() => scrollToSection(item.id)}
                        className="focus-ring flex-none rounded-[10px] px-3 py-1.5 text-[11.5px] font-semibold transition-colors duration-150 ease-out"
                        style={{
                          color: active ? "var(--accent-ink)" : "var(--ink-2)",
                          background: active
                            ? "var(--accent-fill)"
                            : "var(--subtle-fill)",
                        }}
                      >
                        {item.label}
                      </button>
                    );
                  })}
                </div>
              </div>

              <div
                ref={contentRef}
                className="scroll-area min-h-0 flex-1 bg-[var(--popover-bg)] px-4 pb-[calc(100vh-140px)] pt-5 sm:px-8 lg:px-10 lg:pt-8"
                data-testid="app-settings-content"
              >
                <div className="mx-auto w-full max-w-[780px] space-y-4">
                  {error && <ErrorBar message={error} />}
                  {notice && (
                    <div
                      className="rounded-[10px] px-3 py-2 text-[12px] text-ink-2"
                      style={{
                        background: "var(--success-fill)",
                        boxShadow: "inset 0 0 0 0.5px rgba(52, 199, 89, 0.22)",
                      }}
                      data-testid="settings-notice"
                    >
                      {notice}
                    </div>
                  )}
                  <section
                    ref={setSectionRef("appearance")}
                    className="scroll-mt-4 rounded-[14px] p-4"
                    style={sectionPanelStyle}
                  >
                    <div className="flex items-start gap-4">
                      <div className="min-w-0 flex-1">
                        <div className="text-[13px] font-medium text-ink">
                          Appearance
                        </div>
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
                              style={{
                                color: active ? "var(--ink)" : "var(--ink-2)",
                              }}
                              data-testid={`theme-mode-${option.id}`}
                            >
                              {active && (
                                <motion.span
                                  layoutId="theme-mode-thumb"
                                  transition={{
                                    type: "spring",
                                    stiffness: 400,
                                    damping: 32,
                                  }}
                                  className="segmented-thumb absolute inset-0 rounded-[8px]"
                                />
                              )}
                              <span className="relative z-10">
                                {option.label}
                              </span>
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  </section>

                  <section
                    ref={setSectionRef("repos")}
                    className="scroll-mt-4 rounded-[14px] p-4"
                    style={sectionPanelStyle}
                    data-testid="settings-repositories-section"
                  >
                    <div className="mb-3">
                      <div className="text-[13px] font-medium text-ink">
                        Repositories
                      </div>
                      <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
                        统一调整每个仓库的 agent 说明、索引更新和渲染预算。
                      </div>
                    </div>

                    {repos.length === 0 ? (
                      <div
                        className="rounded-[12px] px-3 py-8 text-center text-[12.5px] text-ink-3"
                        style={{
                          background: "var(--subtle-fill-2)",
                          boxShadow: "inset 0 0 0 0.5px var(--hairline)",
                        }}
                      >
                        No repositories yet
                      </div>
                    ) : (
                      <div className="grid gap-3 lg:grid-cols-[220px_minmax(0,1fr)]">
                        <div
                          className="scroll-area max-h-[520px] rounded-[12px] p-1"
                          style={{
                            background: "var(--subtle-fill)",
                            boxShadow: "inset 0 0 0 0.5px var(--hairline)",
                          }}
                          data-testid="settings-repo-list"
                        >
                          {repos.map((repo) => {
                            const active = repo.id === repoSettingsRepo?.id;
                            return (
                              <button
                                key={repo.id}
                                type="button"
                                onClick={() => selectRepo(repo.id)}
                                className="focus-ring flex w-full min-w-0 items-center gap-2 rounded-[10px] px-2.5 py-2 text-left transition-colors duration-150 ease-out"
                                style={{
                                  background: active
                                    ? "var(--popover-bg)"
                                    : "transparent",
                                  boxShadow: active
                                    ? "var(--shadow-float)"
                                    : "none",
                                }}
                                data-testid={`settings-repo-select-${repo.id}`}
                              >
                                <span className={`beacon ${repo.status}`} />
                                <span className="min-w-0 flex-1">
                                  <span
                                    className="block truncate text-[12.5px] font-semibold"
                                    style={{
                                      color: active
                                        ? "var(--accent)"
                                        : "var(--ink)",
                                    }}
                                  >
                                    {repo.name}
                                  </span>
                                  <span className="block truncate text-[10.5px] text-ink-3">
                                    {repo.source.kind}
                                    {repo.description ? " · described" : ""}
                                  </span>
                                </span>
                              </button>
                            );
                          })}
                        </div>
                        <div className="min-w-0">
                          {repoSettingsRepo && (
                            <div className="mb-3 flex items-center gap-2">
                              <div className="min-w-0 flex-1">
                                <div className="truncate text-[13px] font-semibold text-ink">
                                  {repoSettingsRepo.name}
                                </div>
                                <div
                                  className="mono mt-0.5 truncate text-[10.5px] text-ink-3"
                                  title={
                                    repoSettingsRepo.source.kind === "git"
                                      ? (repoSettingsRepo.source.url ??
                                        "git repository")
                                      : repoSettingsRepo.source.kind === "zip"
                                        ? "zip import"
                                        : repoSettingsRepo.path
                                  }
                                >
                                  {repoSettingsRepo.source.kind === "git"
                                    ? (repoSettingsRepo.source.url ??
                                      "git repository")
                                    : repoSettingsRepo.source.kind === "zip"
                                      ? "zip import"
                                      : repoSettingsRepo.path}
                                </div>
                              </div>
                              <span
                                className="rounded-[6px] px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-ink-2"
                                style={{ background: "var(--subtle-fill)" }}
                              >
                                {repoSettingsRepo.source.kind}
                              </span>
                            </div>
                          )}
                          <RepoSettingsPanel repo={repoSettingsRepo} />
                        </div>
                      </div>
                    )}
                  </section>

                  <section
                    ref={setSectionRef("indexing")}
                    className="scroll-mt-4 rounded-[14px] p-4"
                    style={sectionPanelStyle}
                  >
                    <div className="flex items-start gap-4">
                      <div className="min-w-0 flex-1">
                        <div className="text-[13px] font-medium text-ink">
                          Indexing timeout
                        </div>
                        <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
                          全局索引时间预算；环境变量 AKA_INDEX_MAX_SECS
                          会临时覆盖此设置。
                        </div>
                      </div>
                      <span className="cmd-input flex h-8 w-[96px] flex-none items-center px-2.5">
                        <input
                          type="number"
                          min={INDEX_MAX_MIN}
                          max={INDEX_MAX_LIMIT}
                          step={10}
                          value={indexMaxSecs}
                          onChange={(e) =>
                            setIndexMaxSecs(Number(e.target.value))
                          }
                          onBlur={() =>
                            setIndexMaxSecs(clampIndexMaxSecs(indexMaxSecs))
                          }
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
                      <span>
                        {formatDuration(clampIndexMaxSecs(indexMaxSecs))}
                      </span>
                      <button
                        type="button"
                        onClick={() => setIndexMaxSecs(INDEX_MAX_DEFAULT)}
                        disabled={
                          settingsBusy || indexMaxSecs === INDEX_MAX_DEFAULT
                        }
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
                            : {
                                boxShadow:
                                  "inset 0 0 0 0.5px var(--hairline-strong)",
                              }
                        }
                        data-testid="index-max-secs-save"
                      >
                        {settingsBusy ? "Saving..." : "Save"}
                      </button>
                    </div>
                    <div className="themed-divider mt-4 border-t pt-4">
                      <div className="flex items-start gap-4">
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <div className="text-[13px] font-medium text-ink">
                              Open-source analyzer enrichment
                            </div>
                            <span
                              className="rounded-[6px] px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide"
                              style={{
                                background: ossAnalyzerEnrichmentEnabled
                                  ? "var(--accent-fill)"
                                  : "var(--subtle-fill)",
                                color: ossAnalyzerEnrichmentEnabled
                                  ? "var(--accent)"
                                  : "var(--ink-3)",
                              }}
                            >
                              {ossAnalyzerEnrichmentEnabled ? "Enabled" : "Off"}
                            </span>
                          </div>
                          <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
                            仅导入成熟开源分析器结果；下一次导入或更新时生效，跳过或失败不影响
                            graph/search ready。
                          </div>
                        </div>
                        <button
                          type="button"
                          role="switch"
                          aria-checked={ossAnalyzerEnrichmentEnabled}
                          onClick={() =>
                            setOssAnalyzerEnrichmentEnabled((v) => !v)
                          }
                          disabled={settingsBusy}
                          className={`focus-ring relative h-7 w-12 flex-none rounded-full transition-colors duration-150 ease-out ${
                            ossAnalyzerEnrichmentEnabled
                              ? "bg-[var(--accent)]"
                              : "bg-[var(--glass-strong)]"
                          }`}
                          style={{
                            boxShadow:
                              "inset 0 0 0 0.5px var(--hairline-strong)",
                          }}
                          data-testid="oss-analyzer-enrichment-switch"
                        >
                          <span
                            className={`absolute top-1 h-5 w-5 rounded-full bg-white shadow-sm transition-transform duration-150 ease-out ${
                              ossAnalyzerEnrichmentEnabled
                                ? "translate-x-6"
                                : "translate-x-1"
                            }`}
                          />
                        </button>
                      </div>
                      <div
                        className="mt-3 grid grid-cols-2 gap-1.5"
                        data-testid="oss-analyzer-allowlist"
                      >
                        {OSS_ANALYZERS.map((analyzer) => (
                          <div
                            key={analyzer.name}
                            className="min-w-0 rounded-[8px] px-2.5 py-1.5"
                            style={{
                              background: "var(--subtle-fill-2)",
                              boxShadow: "inset 0 0 0 0.5px var(--hairline)",
                            }}
                          >
                            <div className="truncate text-[11.5px] font-medium text-ink">
                              {analyzer.name}
                            </div>
                            <div className="truncate text-[10.5px] text-ink-3">
                              {analyzer.detail}
                            </div>
                          </div>
                        ))}
                      </div>
                      <div className="mt-2 text-[11px] leading-relaxed text-ink-3">
                        SCIP 指 Sourcegraph Code Intelligence
                        Protocol，不是同名优化器。 AKA 只读取显式配置的
                        index.scip 或 aka-facts bundle，不自动启动语言服务。
                      </div>
                      <div className="mt-3 flex items-center gap-3">
                        <span className="cmd-input flex h-8 w-[96px] flex-none items-center px-2.5">
                          <input
                            type="number"
                            min={OSS_ANALYZER_MAX_MIN}
                            max={OSS_ANALYZER_MAX_LIMIT}
                            step={5}
                            value={ossAnalyzerEnrichmentMaxSecs}
                            onChange={(e) =>
                              setOssAnalyzerEnrichmentMaxSecs(
                                Number(e.target.value),
                              )
                            }
                            onBlur={() =>
                              setOssAnalyzerEnrichmentMaxSecs(
                                clampOssAnalyzerMaxSecs(
                                  ossAnalyzerEnrichmentMaxSecs,
                                ),
                              )
                            }
                            disabled={
                              settingsBusy || !ossAnalyzerEnrichmentEnabled
                            }
                            className="tabular h-full w-full text-[12.5px]"
                            data-testid="oss-analyzer-enrichment-max-secs-input"
                          />
                        </span>
                        <span className="text-[11.5px] text-ink-3">
                          {formatDuration(
                            clampOssAnalyzerMaxSecs(
                              ossAnalyzerEnrichmentMaxSecs,
                            ),
                          )}{" "}
                          max per optional pass
                        </span>
                      </div>
                      <label className="mt-3 block">
                        <span className="mb-1 block text-[11.5px] text-ink-3">
                          Existing SCIP index path
                        </span>
                        <span className="cmd-input flex h-8 items-center px-2.5">
                          <input
                            type="text"
                            value={scipIndexPath}
                            onChange={(e) => setScipIndexPath(e.target.value)}
                            disabled={
                              settingsBusy || !ossAnalyzerEnrichmentEnabled
                            }
                            placeholder="repo/index.scip"
                            className="h-full w-full text-[12.5px]"
                            data-testid="scip-index-path-input"
                          />
                        </span>
                      </label>
                      <label className="mt-3 block">
                        <span className="mb-1 block text-[11.5px] text-ink-3">
                          External aka-facts bundle path
                        </span>
                        <span className="cmd-input flex h-8 items-center px-2.5">
                          <input
                            type="text"
                            value={ossAnalyzerFactsPath}
                            onChange={(e) =>
                              setOssAnalyzerFactsPath(e.target.value)
                            }
                            disabled={
                              settingsBusy || !ossAnalyzerEnrichmentEnabled
                            }
                            placeholder="repo/.aka/oss-analyzer-facts.json"
                            className="h-full w-full text-[12.5px]"
                            data-testid="oss-analyzer-facts-path-input"
                          />
                        </span>
                      </label>
                    </div>
                  </section>

                  <section
                    ref={setSectionRef("updates")}
                    className="scroll-mt-4 rounded-[14px] p-4"
                    style={sectionPanelStyle}
                  >
                    <div className="mb-3 flex items-start gap-4">
                      <div className="min-w-0 flex-1">
                        <div className="text-[13px] font-medium text-ink">
                          Updates
                        </div>
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
                        value={
                          nativeUpdate?.latestVersion ??
                          release?.latestVersion ??
                          "Not checked"
                        }
                      />
                    </div>

                    {nativeUpdate && (
                      <div
                        className="mt-3 rounded-[10px] p-3 text-[12px]"
                        style={{
                          boxShadow: "inset 0 0 0 0.5px var(--hairline)",
                        }}
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
                            {nativeUpdate.available
                              ? "Update ready to install"
                              : "Up to date"}
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
                                boxShadow:
                                  "inset 0 0 0 0.5px var(--hairline-strong)",
                              }}
                              data-testid="install-app-update"
                            >
                              {installingUpdate
                                ? "Installing..."
                                : "Install and restart"}
                            </button>
                          </>
                        )}
                      </div>
                    )}

                    {release && (
                      <div
                        className="mt-3 rounded-[10px] p-3 text-[12px]"
                        style={{
                          boxShadow: "inset 0 0 0 0.5px var(--hairline)",
                        }}
                        data-testid="app-update-result"
                      >
                        <div className="mb-2 flex items-center gap-2">
                          <span
                            className="h-2 w-2 rounded-full"
                            style={{
                              background: release.hasUpdate
                                ? "var(--beacon)"
                                : "var(--success)",
                            }}
                          />
                          <span className="font-medium text-ink">
                            {release.hasUpdate
                              ? "Update available"
                              : "Up to date"}
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
                                current={
                                  asset.platform === release.currentPlatform
                                }
                                disabled={openingUrl !== null}
                                opening={openingUrl === asset.url}
                                onOpen={() => void openReleaseUrl(asset.url)}
                              />
                            ))}
                          </div>
                        ) : (
                          <div className="text-[11.5px] text-ink-3">
                            No macOS DMG or Windows EXE package was listed in
                            the manifest.
                          </div>
                        )}

                        {release.hasUpdate && release.releaseUrl && (
                          <button
                            type="button"
                            disabled={openingUrl !== null}
                            onClick={() =>
                              void openReleaseUrl(release.releaseUrl!)
                            }
                            className="focus-ring mt-2 w-full rounded-[9px] px-3 py-1.5 text-[12px] font-semibold text-ink-2 transition-colors duration-150 ease-out hover:text-ink disabled:cursor-not-allowed disabled:opacity-55"
                            style={{
                              boxShadow: "inset 0 0 0 0.5px var(--hairline)",
                            }}
                            data-testid="open-app-release"
                          >
                            {openingUrl === release.releaseUrl
                              ? "Opening..."
                              : "Open release page"}
                          </button>
                        )}
                      </div>
                    )}
                  </section>

                  <section
                    ref={setSectionRef("plugins")}
                    className="scroll-mt-4 rounded-[14px] p-4"
                    style={sectionPanelStyle}
                  >
                    <div className="mb-3 flex items-start gap-4">
                      <div className="min-w-0 flex-1">
                        <div className="text-[13px] font-medium text-ink">
                          Agent plugins
                        </div>
                        <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
                          一键安装或重装 Claude Code / Codex / OpenCode
                          插件/配置包，默认连接桌面端本地 MCP。
                        </div>
                      </div>
                      <button
                        type="button"
                        disabled={clientIntegrationsBusy}
                        onClick={() => {
                          setClientIntegrationsBusy(true);
                          setError(null);
                          void getClientIntegrationsStatus()
                            .then(setClientIntegrations)
                            .catch((e) =>
                              setError(e instanceof Error ? e.message : String(e)),
                            )
                            .finally(() => setClientIntegrationsBusy(false));
                        }}
                        className="focus-ring rounded-[9px] px-2.5 py-1.5 text-[11.5px] font-semibold transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-45"
                        style={{
                          boxShadow: "inset 0 0 0 0.5px var(--hairline)",
                        }}
                        data-testid="refresh-client-integrations"
                      >
                        {clientIntegrationsBusy ? "Refreshing..." : "Refresh"}
                      </button>
                    </div>

                    <div className="space-y-2">
                      {(clientIntegrations?.clients ?? []).map((client) => (
                        <ClientIntegrationRow
                          key={client.client}
                          client={client}
                          action={clientIntegrationAction}
                          onRun={(reinstall) =>
                            void runClientIntegration(client.client, reinstall)
                          }
                        />
                      ))}
                      {!clientIntegrations && (
                        <div className="text-[11.5px] text-ink-3">
                          {clientIntegrationsBusy
                            ? "Reading plugin status..."
                            : "Click Refresh to read plugin status."}
                        </div>
                      )}
                    </div>
                  </section>

                  <section
                    ref={setSectionRef("data")}
                    className="scroll-mt-4 rounded-[14px] p-4"
                    style={sectionPanelStyle}
                  >
                    <div className="mb-3">
                      <div className="text-[13px] font-medium text-ink">
                        Local data
                      </div>
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
                        background: confirmClear
                          ? "var(--danger-fill)"
                          : "var(--subtle-fill)",
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
                  </section>
                </div>
              </div>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}

function VersionTile({ label, value }: { label: string; value: string }) {
  return (
    <div
      className="rounded-[10px] px-3 py-2"
      style={{ background: "var(--subtle-fill)" }}
    >
      <div className="text-[10.5px] font-semibold uppercase text-ink-3">
        {label}
      </div>
      <div className="mt-0.5 truncate text-[13px] font-medium text-ink">
        {value}
      </div>
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
          <span className="text-[12px] font-medium text-ink">
            {asset.label}
          </span>
          {current && (
            <span
              className="rounded-[6px] px-1.5 py-0.5 text-[10px] font-semibold text-ink-2"
              style={{ background: "var(--subtle-fill)" }}
            >
              this device
            </span>
          )}
        </div>
        <div
          className="mt-0.5 truncate text-[11px] text-ink-3"
          title={asset.name}
        >
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

function ClientIntegrationRow({
  client,
  action,
  onRun,
}: {
  client: ClientIntegrationStatus;
  action: string | null;
  onRun(reinstall: boolean): void;
}) {
  const installKey = `${client.client}:install`;
  const reinstallKey = `${client.client}:reinstall`;
  const busy = action === installKey || action === reinstallKey;
  const disabled = action !== null || !client.available;
  const reinstall = client.installed;
  const buttonLabel = reinstall ? "Reinstall" : "Install";

  return (
    <div
      className="rounded-[10px] px-3 py-2"
      style={{
        background: "var(--subtle-fill-2)",
        boxShadow: "inset 0 0 0 0.5px var(--hairline)",
      }}
      data-testid={`client-integration-${client.client}`}
    >
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="text-[12.5px] font-semibold text-ink">
              {client.label}
            </span>
            <span
              className="rounded-[6px] px-1.5 py-0.5 text-[10px] font-semibold uppercase"
              style={{
                background: client.installed
                  ? "var(--success-fill)"
                  : "var(--subtle-fill)",
                color: client.installed ? "var(--success)" : "var(--ink-3)",
              }}
            >
              {client.installed ? "Installed" : "Not installed"}
            </span>
          </div>
        </div>
        <button
          type="button"
          disabled={disabled}
          onClick={() => onRun(reinstall)}
          className="focus-ring flex-none rounded-[8px] px-2.5 py-1.5 text-[11.5px] font-semibold transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-45"
          style={{
            color: "var(--accent-ink)",
            background: "var(--accent-fill)",
            boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)",
          }}
          data-testid={`install-client-integration-${client.client}`}
        >
          {busy ? "Working..." : buttonLabel}
        </button>
      </div>
      {busy && (
        <div
          className="mt-2 h-1 overflow-hidden rounded-full"
          style={{ background: "var(--subtle-fill)" }}
        >
          <div
            className="h-full w-1/3 rounded-full"
            style={{ background: "var(--accent)" }}
          />
        </div>
      )}
    </div>
  );
}

function UpdateProgressBar({ progress }: { progress: InstallProgress }) {
  const percent =
    progress.totalBytes && progress.totalBytes > 0
      ? Math.min(
          100,
          Math.round((progress.downloadedBytes / progress.totalBytes) * 100),
        )
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
        {percent !== null && (
          <span>{formatBytes(progress.downloadedBytes)}</span>
        )}
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
