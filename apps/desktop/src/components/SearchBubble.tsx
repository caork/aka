import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;
const collapsedRadius = 10;
const expandedRadius = 10;
const collapsedWidth = 36;

export default function SearchBubble() {
  const query = useAppStore((s) => s.query);
  const setQuery = useAppStore((s) => s.setQuery);
  const [searchExpanded, setSearchExpanded] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSearchExpanded(true);
        requestAnimationFrame(() => {
          inputRef.current?.focus();
          inputRef.current?.select();
        });
      }
      if (e.key === "Escape" && searchExpanded) {
        setSearchExpanded(false);
        setQuery("");
        inputRef.current?.blur();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [searchExpanded, setQuery]);

  const openSearch = () => {
    setSearchExpanded(true);
    requestAnimationFrame(() => inputRef.current?.focus());
  };

  return (
    <motion.div
      initial={false}
      animate={{ width: collapsedWidth, borderRadius: collapsedRadius }}
      transition={searchExpanded ? spring : { ...spring, delay: 0.06 }}
      className="relative h-8 shrink-0"
      data-testid="sidebar-search-bubble"
    >
      <button
        type="button"
        aria-label="Open symbol search"
        aria-expanded={searchExpanded}
        onClick={openSearch}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            openSearch();
          }
        }}
        className="cmd-input focus-ring grid h-8 w-9 place-items-center"
        style={{ borderRadius: collapsedRadius }}
      >
        <SearchIcon />
      </button>
      <AnimatePresence initial={false}>
        {searchExpanded && (
          <motion.div
            key="search-popover"
            initial={{ opacity: 0, y: -4, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -4, scale: 0.98 }}
            transition={{ duration: 0.12, ease: "easeOut" }}
            role="search"
            aria-label="Search symbols"
            className="cmd-input absolute right-0 top-[calc(100%+8px)] z-50 flex h-9 w-[232px] items-center gap-2 px-3"
            style={{ borderRadius: expandedRadius }}
            data-testid="global-search-popover"
          >
            <SearchIcon />
            <input
              ref={inputRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onBlur={() => {
                if (!query) setSearchExpanded(false);
              }}
              placeholder="Search symbols…"
              className="h-full min-w-0 flex-1 text-[13px]"
              aria-label="Search symbols"
              data-testid="global-search"
            />
            <span className="kbd flex-none whitespace-nowrap text-[10px]">
              ⌘K
            </span>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  );
}

function SearchIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden className="flex-none text-ink-3">
      <circle cx="11" cy="11" r="7" stroke="currentColor" strokeWidth="2" />
      <path d="m20 20-3.5-3.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}
