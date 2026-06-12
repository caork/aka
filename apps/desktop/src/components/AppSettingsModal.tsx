import { motion } from "framer-motion";
import { useState } from "react";
import { clearDesktopData } from "../repo-api";
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
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

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
      await clearDesktopData();
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
    <Modal open={open} onClose={onClose} title="Settings" width={420}>
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
            选择 aka 的显示模式
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
