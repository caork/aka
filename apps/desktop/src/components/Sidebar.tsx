import { motion } from "framer-motion";
import { useState } from "react";
import { useAppStore } from "../store";
import AppSettingsModal from "./AppSettingsModal";
import ImportRepoModal from "./ImportRepoModal";
import RepoSettingsModal from "./RepoSettingsModal";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

export default function Sidebar() {
  const repos = useAppStore((s) => s.repos);
  const selectedRepoId = useAppStore((s) => s.selectedRepoId);
  const selectRepo = useAppStore((s) => s.selectRepo);
  const [importOpen, setImportOpen] = useState(false);
  const [appSettingsOpen, setAppSettingsOpen] = useState(false);
  const [settingsRepoId, setSettingsRepoId] = useState<string | null>(null);

  const settingsRepo = settingsRepoId
    ? (repos.find((r) => r.id === settingsRepoId) ?? null)
    : null;

  return (
    <motion.aside
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={spring}
      className="glass-panel flex w-[220px] flex-none flex-col rounded-none"
      style={{ borderRadius: 0 }}
      data-testid="sidebar"
    >
      {/* logo */}
      <button
        onClick={() => setAppSettingsOpen(true)}
        className="focus-ring mx-3 mt-4 flex items-center gap-2.5 rounded-[12px] px-2 py-2 text-left transition-colors duration-150 ease-out hover:bg-[var(--hover-fill)]"
        aria-label="Open settings"
        data-testid="app-settings"
      >
        <img
          src="/logo.png"
          alt="aka logo"
          className="app-logo h-7 w-7"
          draggable={false}
        />
        <div className="text-[15px] font-semibold tracking-tight text-ink">aka</div>
      </button>

      {/* repo list */}
      <div className="px-3 pb-1 pt-2 text-[10.5px] font-semibold uppercase tracking-[0.08em] text-ink-3">
        <span className="px-2">Repositories</span>
      </div>
      <nav className="scroll-area flex-1 px-2">
        {repos.map((repo, idx) => {
          const active = repo.id === selectedRepoId;
          return (
            <motion.div
              key={repo.id}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ ...spring, delay: 0.04 + idx * 0.02 }}
              role="button"
              tabIndex={0}
              onClick={() => selectRepo(repo.id)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  selectRepo(repo.id);
                }
              }}
              className="focus-ring group relative mb-0.5 flex w-full cursor-pointer items-center gap-2.5 rounded-[10px] px-3 py-2 text-left transition-colors duration-150 ease-out"
              style={{
                background: active ? "var(--accent-fill)" : "transparent",
              }}
              data-testid={`repo-row-${repo.id}`}
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
                  className="block truncate text-[11px]"
                  style={{
                    color: repo.status === "failed" ? "var(--danger-ink)" : undefined,
                  }}
                >
                  <span className={repo.status === "failed" ? "" : "text-ink-3"}>
                    {repo.status === "indexing"
                      ? "indexing…"
                      : repo.status === "failed"
                        ? (repo.detail ?? "索引失败")
                        : repo.status === "idle"
                          ? "not indexed"
                          : `${repo.symbols.toLocaleString()} symbols`}
                  </span>
                </span>
              </span>

              <button
                aria-label={`${repo.name} settings`}
                onClick={(e) => {
                  e.stopPropagation();
                  setSettingsRepoId(repo.id);
                }}
                className="focus-ring absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-[7px] text-ink-3 opacity-0 transition-all duration-150 ease-out hover:text-ink focus-visible:opacity-100 group-hover:opacity-100"
                style={{ background: "var(--glass-bg-strong)" }}
                data-testid={`repo-settings-${repo.id}`}
              >
                <GearIcon size={13} />
              </button>
            </motion.div>
          );
        })}
      </nav>

      {/* add repository */}
      <div className="themed-divider border-t p-3">
        <button
          onClick={() => setImportOpen(true)}
          className="glass focus-ring flex w-full items-center justify-center gap-2 rounded-[10px] px-3 py-2 text-[13px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:text-[var(--accent)]"
          data-testid="add-repository"
        >
          <PlusIcon />
          Add repository
        </button>
      </div>

      <ImportRepoModal open={importOpen} onClose={() => setImportOpen(false)} />
      <AppSettingsModal
        open={appSettingsOpen}
        onClose={() => setAppSettingsOpen(false)}
      />
      <RepoSettingsModal
        repo={settingsRepo}
        onClose={() => setSettingsRepoId(null)}
      />
    </motion.aside>
  );
}

function PlusIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden>
      <path d="M12 5v14M5 12h14" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" />
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
