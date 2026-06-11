import { motion } from "framer-motion";
import { useAppStore } from "../store";
import type { ThemeMode } from "../theme";
import Modal from "./Modal";

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

  return (
    <Modal open={open} onClose={onClose} title="Settings" width={420}>
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
    </Modal>
  );
}
