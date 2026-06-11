import { AnimatePresence, LayoutGroup, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

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

  const collapseSearch = () => {
    if (!query) setSearchExpanded(false);
  };

  return (
    <LayoutGroup>
      <motion.div
        layout
        transition={spring}
        onClick={searchExpanded ? undefined : openSearch}
        style={{ borderRadius: searchExpanded ? 10 : 18 }}
        className={[
          "cmd-input relative flex h-9 shrink-0 items-center overflow-hidden",
          searchExpanded
            ? "w-[260px] cursor-text gap-2.5 px-3"
            : "w-9 cursor-pointer justify-center",
        ].join(" ")}
        data-testid="sidebar-search-bubble"
      >
        <SearchIcon />
        <AnimatePresence>
          {searchExpanded && (
            <motion.input
              key="input"
              ref={inputRef}
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.12 }}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onBlur={collapseSearch}
              placeholder="Search symbols…"
              className="h-full min-w-0 flex-1 text-[13px]"
              data-testid="global-search"
            />
          )}
        </AnimatePresence>
        <AnimatePresence>
          {searchExpanded && (
            <motion.span
              key="kbd"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.1 }}
              className="kbd flex-none text-[10px]"
            >
              ⌘K
            </motion.span>
          )}
        </AnimatePresence>
      </motion.div>
    </LayoutGroup>
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
