import { AnimatePresence, motion } from "framer-motion";
import { useRef, useState } from "react";
import { compactIndexStatus } from "../index-log";
import { useAppStore } from "../store";
import AppSettingsModal from "./AppSettingsModal";
import ImportRepoModal from "./ImportRepoModal";
import RepoSettingsModal from "./RepoSettingsModal";

const spring = { type: "spring", stiffness: 320, damping: 30 } as const;

export default function RepoDropdown() {
  const repos = useAppStore((s) => s.repos);
  const selectedRepoId = useAppStore((s) => s.selectedRepoId);
  const selectRepo = useAppStore((s) => s.selectRepo);
  const [open, setOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [appSettingsOpen, setAppSettingsOpen] = useState(false);
  const [settingsRepoId, setSettingsRepoId] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const settingsRepo = settingsRepoId
    ? (repos.find((r) => r.id === settingsRepoId) ?? null)
    : null;

  const enter = () => {
    if (timer.current) clearTimeout(timer.current);
    setOpen(true);
  };
  const leave = () => {
    timer.current = setTimeout(() => setOpen(false), 100);
  };

  return (
    <div className="relative" onMouseEnter={enter} onMouseLeave={leave}>
      {/* Logo button */}
      <motion.div
        role="button"
        tabIndex={0}
        aria-label="Open settings"
        data-testid="app-settings"
        onClick={() => {
          setAppSettingsOpen(true);
          setOpen(false);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setAppSettingsOpen(true);
            setOpen(false);
          }
        }}
        className="focus-ring flex h-9 w-9 cursor-pointer items-center justify-center rounded-[10px]"
        animate={{
          background: open ? "var(--accent-fill)" : "var(--glass-bg)",
        }}
        transition={{ duration: 0.15 }}
        style={{
          backdropFilter: "blur(8px)",
          WebkitBackdropFilter: "blur(8px)",
          boxShadow:
            "inset 0 0 0 0.5px var(--glass-inner), var(--shadow-float)",
        }}
      >
        <motion.img
          src="/logo.png"
          alt="AKA"
          className="app-logo h-[18px] w-[18px]"
          draggable={false}
          animate={{ scale: open ? 1.08 : 1 }}
          transition={spring}
        />
      </motion.div>

      {/* Dropdown */}
      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, y: 8, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 8, scale: 0.96 }}
            transition={spring}
            className="absolute bottom-[calc(100%+6px)] left-0 z-50 w-[220px] origin-bottom-left overflow-hidden rounded-[14px]"
            style={{
              background: "var(--popover-bg)",
              boxShadow:
                "inset 0 0 0 1px var(--hairline-strong), var(--shadow-panel)",
            }}
            onMouseEnter={enter}
            onMouseLeave={leave}
          >
            <nav className="p-1.5">
              {repos.length === 0 && (
                <div className="px-3 py-3 text-[12px] leading-relaxed text-ink-3">
                  No repositories yet
                </div>
              )}
              {repos.map((repo, idx) => {
                const active = repo.id === selectedRepoId;
                return (
                  <motion.div
                    key={repo.id}
                    initial={{ opacity: 0, x: -4 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{ ...spring, delay: idx * 0.025 }}
                    role="button"
                    tabIndex={0}
                    onClick={() => {
                      selectRepo(repo.id);
                      setOpen(false);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        selectRepo(repo.id);
                        setOpen(false);
                      }
                    }}
                    className="group relative flex w-full cursor-pointer items-center gap-2.5 rounded-[10px] px-3 py-2 text-left transition-colors duration-100 hover:bg-[var(--hover-fill)]"
                    style={{
                      background: active ? "var(--accent-fill)" : undefined,
                    }}
                    data-testid={`repo-dropdown-${repo.id}`}
                  >
                    <span className={`beacon ${repo.status}`} />
                    <span className="min-w-0 flex-1">
                      <span
                        className={`block truncate text-[13px] font-medium ${
                          active ? "text-[var(--accent)]" : "text-ink"
                        }`}
                      >
                        {repo.name}
                      </span>
                      <span
                        className={`block truncate text-[11px] ${
                          repo.status === "failed" ? "text-red-500" : "text-ink-3"
                        }`}
                      >
                        {repo.status === "indexing"
                          ? compactIndexStatus(repo)
                          : repo.status === "failed"
                            ? (repo.detail ?? "failed")
                            : repo.status === "idle"
                              ? "not indexed"
                              : `${repo.symbols.toLocaleString()} symbols`}
                      </span>
                    </span>
                    <button
                      aria-label={`${repo.name} settings`}
                      onClick={(e) => {
                        e.stopPropagation();
                        setSettingsRepoId(repo.id);
                        setOpen(false);
                      }}
                      className="focus-ring absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-[7px] text-ink-3 opacity-0 transition-all duration-150 hover:bg-[var(--hover-fill-strong)] hover:text-ink focus-visible:opacity-100 group-hover:opacity-100"
                      style={{ background: "var(--glass-bg-strong)" }}
                    >
                      <GearIcon size={13} />
                    </button>
                  </motion.div>
                );
              })}
            </nav>
            <div className="border-t border-[var(--hairline)] p-1.5">
              <button
                onClick={() => {
                  setImportOpen(true);
                  setOpen(false);
                }}
                className="focus-ring flex w-full items-center gap-2 rounded-[10px] px-3 py-2 text-[13px] font-medium text-ink-2 transition-colors duration-100 hover:bg-[var(--hover-fill)] hover:text-[var(--accent)]"
                data-testid="repo-dropdown-add"
              >
                <PlusIcon />
                Add repository
              </button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      <ImportRepoModal open={importOpen} onClose={() => setImportOpen(false)} />
      <AppSettingsModal
        open={appSettingsOpen}
        onClose={() => setAppSettingsOpen(false)}
      />
      <RepoSettingsModal repo={settingsRepo} onClose={() => setSettingsRepoId(null)} />
    </div>
  );
}

function PlusIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden>
      <path
        d="M12 5v14M5 12h14"
        stroke="currentColor"
        strokeWidth="2.2"
        strokeLinecap="round"
      />
    </svg>
  );
}

function GearIcon({ size = 15 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden>
      <path d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z" stroke="currentColor" strokeWidth="1.7" />
      <path
        d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h.01a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v.01a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z"
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
