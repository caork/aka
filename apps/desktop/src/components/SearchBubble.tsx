import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;
const collapsedRadius = 18;
const expandedRadius = 10;
const collapsedWidth = 36;
const expandedWidth = 232;

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
    <motion.div
      initial={false}
      animate={{
        width: searchExpanded ? expandedWidth : collapsedWidth,
        borderRadius: searchExpanded ? expandedRadius : collapsedRadius,
      }}
      transition={
        searchExpanded
          ? spring
          : { ...spring, delay: 0.06 }
      }
      role={searchExpanded ? "search" : "button"}
      tabIndex={searchExpanded ? -1 : 0}
      aria-label={searchExpanded ? "Search symbols" : "Open symbol search"}
      aria-expanded={searchExpanded}
      onClick={searchExpanded ? undefined : openSearch}
      onKeyDown={(e) => {
        if (searchExpanded) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          openSearch();
        }
      }}
      style={{ transformOrigin: "left center" }}
      className={[
        "cmd-input relative flex h-9 max-w-[calc(100vw-24px)] shrink-0 items-center overflow-hidden",
        searchExpanded ? "cursor-text" : "cursor-pointer",
      ].join(" ")}
      data-testid="sidebar-search-bubble"
    >
      <span className="grid h-9 w-9 flex-none place-items-center">
        <SearchIcon />
      </span>
      <AnimatePresence initial={false}>
        {searchExpanded && (
          <motion.div
            key="search-content"
            initial={{ opacity: 0, x: -4 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -4 }}
            transition={{ duration: 0.12, ease: "easeOut" }}
            className="absolute bottom-0 left-9 top-0 flex w-[196px] items-center gap-2 pr-3"
          >
            <input
              ref={inputRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onBlur={collapseSearch}
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
