import { AnimatePresence, motion } from "framer-motion";
import CodeView from "./CodeView";
import SearchPanel from "./SearchPanel";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

export default function DocView() {
  const codeTarget = useAppStore((s) => s.codeTarget);
  const closeCode = useAppStore((s) => s.closeCode);

  /* no file open — full-width search results */
  if (!codeTarget) {
    return <SearchPanel />;
  }

  /* file open — search results sidebar + code editor */
  return (
    <div className="flex h-full overflow-hidden">
      {/* left: search / symbol list */}
      <div
        className="flex h-full w-[280px] flex-none flex-col border-r"
        style={{ borderColor: "rgba(15,23,42,0.07)" }}
      >
        <SearchPanel compact />
      </div>

      {/* right: code editor */}
      <div className="relative min-w-0 flex-1">
        {/* breadcrumb + close */}
        <div
          className="flex h-9 flex-none items-center gap-2 border-b px-4"
          style={{
            borderColor: "rgba(15,23,42,0.07)",
            background: "rgba(255,255,255,0.5)",
          }}
        >
          <FileIcon />
          <span className="mono min-w-0 flex-1 truncate text-[12px] text-ink-2">
            {codeTarget.path}
          </span>
          <button
            onClick={closeCode}
            aria-label="Close file"
            className="focus-ring flex h-5 w-5 flex-none items-center justify-center rounded-[6px] text-[13px] text-ink-3 transition-colors hover:bg-[rgba(15,23,42,0.06)] hover:text-ink"
          >
            ×
          </button>
        </div>

        <AnimatePresence mode="wait">
          <motion.div
            key={codeTarget.repo + codeTarget.path}
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={spring}
            className="h-[calc(100%-36px)]"
          >
            <CodeView />
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}

function FileIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden className="flex-none opacity-50">
      <path d="M14 3H7a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V7l-4-4Z" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" />
      <path d="M14 3v4h4" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" />
    </svg>
  );
}
