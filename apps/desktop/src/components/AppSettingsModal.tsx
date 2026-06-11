import { motion } from "framer-motion";
import { useState } from "react";
import { clearAppData } from "../repo-api";
import { useAppStore } from "../store";
import { refreshRepos } from "../store";
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
  const [clearing, setClearing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const clearData = async () => {
    const ok = window.confirm(
      "清理后会移除桌面版已导入仓库、索引和托管 checkout；不会删除你手动选择的本地源码目录。确定继续吗？",
    );
    if (!ok) return;
    setClearing(true);
    setError(null);
    setNotice(null);
    try {
      await clearAppData();
      window.localStorage.removeItem("aka.selectedRepo");
      await refreshRepos();
      setNotice("桌面版应用数据已清理");
    } catch (e) {
      setError(e instanceof Error ? e.message : "清理失败");
    } finally {
      setClearing(false);
    }
  };

  return (
    <Modal open={open} onClose={onClose} title="Settings" width={420}>
      <div className="space-y-5">
        {error && <ErrorBar message={error} />}
        {notice && (
          <div
            className="rounded-[10px] px-3 py-2 text-[12px] text-[var(--success-ink)]"
            style={{ background: "var(--success-fill)" }}
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

        <div className="themed-divider border-t pt-4">
          <div className="mb-3">
            <div className="text-[13px] font-medium text-ink">App Data</div>
            <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
              清理桌面版注册表、索引与托管 checkout
            </div>
          </div>
          <button
            type="button"
            onClick={() => void clearData()}
            disabled={clearing}
            className="focus-ring rounded-[9px] px-3 py-2 text-[12.5px] font-semibold transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-55"
            style={{
              color: "var(--danger-ink)",
              background: "var(--danger-fill)",
              boxShadow: "inset 0 0 0 0.5px var(--danger-border)",
            }}
            data-testid="clear-app-data"
          >
            {clearing ? "清理中…" : "一键清理"}
          </button>
        </div>
      </div>
    </Modal>
  );
}
