import { motion } from "framer-motion";
import { useState } from "react";
import { compactIndexStatus } from "../index-log";
import { useAppStore } from "../store";
import AppSettingsModal from "./AppSettingsModal";
import ImportRepoModal from "./ImportRepoModal";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

export default function Sidebar() {
  const repos = useAppStore((s) => s.repos);
  const selectedRepoId = useAppStore((s) => s.selectedRepoId);
  const selectRepo = useAppStore((s) => s.selectRepo);
  const [importOpen, setImportOpen] = useState(false);
  const [appSettingsOpen, setAppSettingsOpen] = useState(false);

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
          alt="AKA logo"
          className="app-logo h-7 w-7"
          draggable={false}
        />
        <div className="text-[15px] font-semibold tracking-tight text-ink">AKA</div>
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
              className="focus-ring relative mb-0.5 flex w-full cursor-pointer items-center gap-2.5 rounded-[10px] px-3 py-2 text-left transition-colors duration-150 ease-out"
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
                      ? compactIndexStatus(repo)
                      : repo.status === "failed"
                        ? (repo.detail ?? "索引失败")
                        : repo.status === "idle"
                          ? "not indexed"
                          : `${repo.symbols.toLocaleString()} symbols`}
                  </span>
                </span>
              </span>
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
