import { motion } from "framer-motion";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

export default function Sidebar() {
  const repos = useAppStore((s) => s.repos);
  const selectedRepoId = useAppStore((s) => s.selectedRepoId);
  const selectRepo = useAppStore((s) => s.selectRepo);

  return (
    <motion.aside
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={spring}
      className="glass-panel m-3 mr-0 flex w-[220px] flex-none flex-col"
      data-testid="sidebar"
    >
      {/* logo */}
      <div className="flex items-center gap-2.5 px-5 pb-4 pt-6">
        <div
          className="flex h-7 w-7 items-center justify-center rounded-[9px] text-[13px] font-bold text-white"
          style={{
            background: "linear-gradient(135deg, #2e7cf6, #5a9bff)",
            boxShadow:
              "0 0 0 1px rgba(46,124,246,.28), 0 0 20px rgba(46,124,246,.16)",
          }}
        >
          a
        </div>
        <div className="text-[15px] font-semibold tracking-tight text-ink">
          aka
        </div>
      </div>

      {/* repo list */}
      <div className="px-3 pb-1 pt-2 text-[10.5px] font-semibold uppercase tracking-[0.08em] text-ink-3">
        <span className="px-2">Repositories</span>
      </div>
      <nav className="scroll-area flex-1 px-2">
        {repos.map((repo, idx) => {
          const active = repo.id === selectedRepoId;
          return (
            <motion.button
              key={repo.id}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ ...spring, delay: 0.04 + idx * 0.02 }}
              onClick={() => selectRepo(repo.id)}
              className="focus-ring group relative mb-0.5 flex w-full items-center gap-2.5 rounded-[10px] px-3 py-2 text-left transition-colors duration-150 ease-out"
              style={{
                background: active ? "rgba(46,124,246,0.09)" : "transparent",
              }}
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
                <span className="block truncate text-[11px] text-ink-3">
                  {repo.status === "indexing"
                    ? "indexing…"
                    : repo.status === "idle"
                      ? "not indexed"
                      : `${repo.symbols.toLocaleString()} symbols`}
                </span>
              </span>
            </motion.button>
          );
        })}
      </nav>

      {/* settings */}
      <div className="border-t border-[rgba(15,23,42,0.06)] p-3">
        <button className="focus-ring flex w-full items-center gap-2.5 rounded-[10px] px-3 py-2 text-[13px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.04)]">
          <GearIcon />
          Settings
        </button>
      </div>
    </motion.aside>
  );
}

function GearIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" aria-hidden>
      <path
        d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z"
        stroke="currentColor"
        strokeWidth="1.7"
      />
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
