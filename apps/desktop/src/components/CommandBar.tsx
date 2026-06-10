import { motion } from "framer-motion";
import { useEffect, useRef } from "react";
import { useAppStore } from "../store";
import SegmentedControl from "./SegmentedControl";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

export default function CommandBar() {
  const view = useAppStore((s) => s.view);
  const setView = useAppStore((s) => s.setView);
  const query = useAppStore((s) => s.query);
  const setQuery = useAppStore((s) => s.setQuery);
  const embeddingOn = useAppStore((s) => s.embeddingOn);
  const toggleEmbedding = useAppStore((s) => s.toggleEmbedding);
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

      <SegmentedControl value={view} onChange={setView} />

      {/* embedding toggle indicator */}
      <button
        onClick={toggleEmbedding}
        className="focus-ring flex items-center gap-2 rounded-[10px] px-2.5 py-1.5 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.04)]"
        title={
          embeddingOn
            ? "Hybrid search: BM25 + vectors"
            : "BM25 only — embeddings off"
        }
        data-testid="embedding-toggle"
      >
        <span className="text-[11.5px] font-medium text-ink-2">embeddings</span>
        <span
          className="relative h-[18px] w-[30px] rounded-full transition-colors duration-150 ease-out"
          style={{
            background: embeddingOn ? "#2e7cf6" : "rgba(15,23,42,0.12)",
            boxShadow: embeddingOn
              ? "0 0 0 1px rgba(46,124,246,.28), 0 0 20px rgba(46,124,246,.16)"
              : "inset 0 0 0 0.5px rgba(15,23,42,0.08)",
          }}
        >
          <motion.span
            className="absolute top-[2px] h-[14px] w-[14px] rounded-full bg-white"
            style={{ boxShadow: "0 1px 2px rgba(16,24,40,.2)" }}
            animate={{ left: embeddingOn ? 14 : 2 }}
            transition={{ type: "spring", stiffness: 400, damping: 32 }}
          />
        </span>
      </button>
    </motion.header>
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
