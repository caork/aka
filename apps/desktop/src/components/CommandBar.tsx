import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef } from "react";
import { useAppStore } from "../store";
import SegmentedControl from "./SegmentedControl";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

export default function CommandBar() {
  const view = useAppStore((s) => s.view);
  const setView = useAppStore((s) => s.setView);
  const query = useAppStore((s) => s.query);
  const setQuery = useAppStore((s) => s.setQuery);
  const codeTarget = useAppStore((s) => s.codeTarget);
  const closeCode = useAppStore((s) => s.closeCode);
  const inputRef = useRef<HTMLInputElement>(null);

  /* ⌘K focuses the global search */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        inputRef.current?.focus();
        inputRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <motion.header
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ ...spring, delay: 0.03 }}
      className="glass m-3 mb-0 flex flex-none items-center gap-4 px-4 py-2.5"
      data-testid="command-bar"
    >
      {/* global search — the visual protagonist */}
      <div className="cmd-input flex h-9 flex-1 items-center gap-2.5 px-3">
        <SearchIcon />
        <input
          ref={inputRef}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search symbols, files, references…"
          className="h-full flex-1 text-[13.5px]"
          data-testid="global-search"
        />
        <span className="kbd">⌘K</span>
      </div>

      {/* 临时代码页签（类似 GitHub/IDE）：仅当有打开的文件时出现 */}
      <AnimatePresence initial={false}>
        {codeTarget && (
          <motion.div
            initial={{ opacity: 0, scale: 0.96 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.96 }}
            transition={spring}
            role="tablist"
            aria-label="打开的文件"
            className="flex flex-none items-center rounded-[10px] p-0.5"
            style={{
              background: "rgba(15,23,42,0.05)",
              boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.06)",
            }}
            data-testid="code-tab"
          >
            <div
              className="flex items-center gap-0.5 rounded-[8px]"
              style={
                view === "code"
                  ? {
                      background: "#fff",
                      boxShadow:
                        "0 1px 2px rgba(16,24,40,.06), 0 2px 6px rgba(16,24,40,.06), inset 0 0 0 0.5px rgba(15,23,42,.04)",
                    }
                  : undefined
              }
            >
              <button
                role="tab"
                aria-selected={view === "code"}
                onClick={() => setView("code")}
                title={codeTarget.path}
                className="focus-ring flex items-center gap-1.5 rounded-[8px] py-1.5 pl-3 pr-0.5 text-[12.5px] font-medium transition-colors duration-150 ease-out"
                style={{ color: view === "code" ? "#0f172a" : "#475569" }}
                data-testid="code-tab-open"
              >
                <FileIcon />
                <span className="mono max-w-[180px] truncate text-[12px]">
                  {codeTarget.path.split("/").pop() || codeTarget.path}
                </span>
              </button>
              <button
                onClick={closeCode}
                aria-label="关闭代码预览"
                className="focus-ring mr-1 flex h-5 w-5 flex-none items-center justify-center rounded-[6px] text-[13px] leading-none text-ink-3 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.06)] hover:text-ink"
                data-testid="code-tab-close"
              >
                ×
              </button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      <SegmentedControl value={view} onChange={setView} />
    </motion.header>
  );
}

function FileIcon() {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
      className="flex-none opacity-70"
    >
      <path
        d="M14 3H7a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V7l-4-4Z"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinejoin="round"
      />
      <path d="M14 3v4h4" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" />
    </svg>
  );
}

function SearchIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
      className="text-ink-3"
    >
      <circle cx="11" cy="11" r="7" stroke="currentColor" strokeWidth="2" />
      <path
        d="m20 20-3.5-3.5"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}
