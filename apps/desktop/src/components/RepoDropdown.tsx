import { AnimatePresence, motion } from "framer-motion";
import { useRef, useState } from "react";
import { useAppStore } from "../store";
import ImportRepoModal from "./ImportRepoModal";
import RepoSettingsModal from "./RepoSettingsModal";

const spring = { type: "spring", stiffness: 320, damping: 30 } as const;

export default function RepoDropdown() {
  const repos = useAppStore((s) => s.repos);
  const selectedRepoId = useAppStore((s) => s.selectedRepoId);
  const selectRepo = useAppStore((s) => s.selectRepo);
  const [open, setOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
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
        className="flex h-9 w-9 cursor-default items-center justify-center rounded-[10px]"
        animate={{
          background: open ? "rgba(46,124,246,0.1)" : "rgba(255,255,255,0.55)",
        }}
        transition={{ duration: 0.15 }}
        style={{
          backdropFilter: "blur(8px)",
          WebkitBackdropFilter: "blur(8px)",
          boxShadow:
            "inset 0 0 0 0.5px rgba(15,23,42,0.07), 0 1px 3px rgba(16,24,40,.06)",
        }}
      >
        <motion.img
          src="/logo.png"
          alt="aka"
          className="h-[18px] w-[18px]"
          draggable={false}
          animate={{ scale: open ? 1.08 : 1 }}
          transition={spring}
        />
      </motion.div>

      {/* Dropdown */}
      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, y: -8, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -8, scale: 0.96 }}
            transition={spring}
            className="absolute left-0 top-[calc(100%+6px)] z-50 w-[220px] overflow-hidden rounded-[14px]"
            style={{
              background: "rgba(255,255,255,0.84)",
              backdropFilter: "blur(28px) saturate(190%)",
              WebkitBackdropFilter: "blur(28px) saturate(190%)",
              boxShadow:
                "0 8px 32px rgba(16,24,40,.12), inset 0 1px 0 rgba(255,255,255,0.8), inset 0 0 0 0.5px rgba(15,23,42,0.07)",
            }}
            onMouseEnter={enter}
            onMouseLeave={leave}
          >
            <nav className="p-1.5">
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
                    className="group relative flex w-full cursor-pointer items-center gap-2.5 rounded-[10px] px-3 py-2 text-left transition-colors duration-100 hover:bg-[rgba(15,23,42,0.04)]"
                    style={{
                      background: active ? "rgba(46,124,246,0.09)" : undefined,
                    }}
                    data-testid={`repo-dropdown-${repo.id}`}
                  >
                    <span className={`beacon ${repo.status}`} />
                    <span className="min-w-0 flex-1">
                      <span
                        className={`block truncate text-[13px] font-medium ${
                          active ? "text-[#2e7cf6]" : "text-ink"
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
                          ? "indexing…"
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
                      className="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-[7px] text-ink-3 opacity-0 transition-all duration-150 hover:bg-[rgba(15,23,42,0.06)] hover:text-ink group-hover:opacity-100"
                      style={{ background: "rgba(255,255,255,0.7)" }}
                    >
                      <GearIcon size={13} />
                    </button>
                  </motion.div>
                );
              })}
            </nav>
            <div className="border-t border-[rgba(15,23,42,0.06)] p-1.5">
              <button
                onClick={() => {
                  setImportOpen(true);
                  setOpen(false);
                }}
                className="flex w-full items-center gap-2 rounded-[10px] px-3 py-2 text-[13px] font-medium text-ink-2 transition-colors duration-100 hover:bg-[rgba(15,23,42,0.05)] hover:text-[#2e7cf6]"
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
