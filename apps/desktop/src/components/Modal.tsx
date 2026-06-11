import { AnimatePresence, motion } from "framer-motion";
import { useEffect } from "react";
import { createPortal } from "react-dom";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

/** 液态玻璃 modal 外壳 —— 居中浮窗 + 遮罩，Esc / 点遮罩关闭。 */
export default function Modal({
  open,
  onClose,
  title,
  children,
  width = 440,
}: {
  open: boolean;
  onClose(): void;
  title: React.ReactNode;
  children: React.ReactNode;
  width?: number;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.15, ease: "easeOut" }}
          className="fixed inset-0 z-50 flex items-center justify-center p-6"
          style={{
            background: "var(--modal-overlay)",
            backdropFilter: "blur(6px)",
            WebkitBackdropFilter: "blur(6px)",
          }}
          onPointerDown={(e) => {
            if (e.target === e.currentTarget) onClose();
          }}
          data-testid="modal-overlay"
        >
          <motion.div
            initial={{ opacity: 0, y: 16, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 12, scale: 0.98 }}
            transition={spring}
            className="glass-panel flex max-h-full flex-col overflow-hidden"
            style={{ width, maxWidth: "100%" }}
            role="dialog"
            aria-modal="true"
          >
            <div className="flex items-center gap-2 px-5 pb-3 pt-4">
              <div className="min-w-0 flex-1 text-[14.5px] font-semibold tracking-tight text-ink">
                {title}
              </div>
              <button
                onClick={onClose}
                aria-label="Close dialog"
                className="themed-hover focus-ring -mr-1 flex h-6 w-6 flex-none items-center justify-center rounded-[7px] text-[15px] leading-none text-ink-3 transition-colors duration-150 ease-out hover:text-ink"
              >
                ×
              </button>
            </div>
            <div className="scroll-area min-h-0 flex-1 px-5 pb-5">{children}</div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}

/** modal 内的玻璃红条错误提示。 */
export function ErrorBar({ message }: { message: string }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: -4 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.15, ease: "easeOut" }}
      className="mb-3 flex items-start gap-2 rounded-[10px] px-3 py-2 text-[12px]"
      style={{
        background: "var(--danger-fill)",
        boxShadow: "inset 0 0 0 0.5px var(--danger-border)",
        color: "var(--danger-ink)",
      }}
      data-testid="modal-error"
    >
      <span
        className="mt-[3px] h-1.5 w-1.5 flex-none rounded-full"
        style={{ background: "var(--danger)" }}
      />
      <span className="min-w-0 break-words">{message}</span>
    </motion.div>
  );
}

/** 表单标签 + 输入框样式（沿用 cmd-input 玻璃语言）。 */
export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="mb-3 block">
      <span className="mb-1.5 flex items-baseline gap-2 text-[12px] font-medium text-ink-2">
        {label}
        {hint && <span className="text-[11px] font-normal text-ink-3">{hint}</span>}
      </span>
      {children}
    </label>
  );
}

export function TextInput(props: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <span className="cmd-input flex h-9 items-center px-3">
      <input {...props} className="h-full w-full flex-1 text-[13px]" />
    </span>
  );
}
