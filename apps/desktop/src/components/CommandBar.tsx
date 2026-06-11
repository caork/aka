import { AnimatePresence, LayoutGroup, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { useAppStore } from "../store";
import SegmentedControl from "./SegmentedControl";


const spring = { type: "spring", stiffness: 320, damping: 28 } as const;

export default function CommandBar() {
  const view = useAppStore((s) => s.view);
  const setView = useAppStore((s) => s.setView);
  const query = useAppStore((s) => s.query);
  const setQuery = useAppStore((s) => s.setQuery);
  const inputRef = useRef<HTMLInputElement>(null);
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setExpanded(true);
        requestAnimationFrame(() => {
          inputRef.current?.focus();
          inputRef.current?.select();
        });
      }
      if (e.key === "Escape" && expanded) {
        setExpanded(false);
        setQuery("");
        inputRef.current?.blur();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [expanded, setQuery]);

  const open = () => {
    setExpanded(true);
    requestAnimationFrame(() => inputRef.current?.focus());
  };

  const collapse = () => {
    if (!query) setExpanded(false);
  };

  return (
    <motion.header
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ ...spring, delay: 0.03 }}
      className="glass m-3 mb-0 flex flex-none items-center gap-3 px-3 py-2.5"
      data-testid="command-bar"
    >
      <LayoutGroup>
        {/* search bubble ↔ expanded bar */}
        <motion.div
          layout
          transition={spring}
          onClick={expanded ? undefined : open}
          style={{ borderRadius: expanded ? 10 : 18 }}
          className={[
            "cmd-input relative flex h-9 shrink-0 items-center overflow-hidden",
            expanded
              ? "flex-1 cursor-text gap-2.5 px-3"
              : "w-9 cursor-pointer justify-center",
          ].join(" ")}
          data-testid="command-bar-bubble"
        >
          <SearchIcon />

          <AnimatePresence>
            {expanded && (
              <motion.input
                key="input"
                ref={inputRef}
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.12 }}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onBlur={collapse}
                placeholder="Search symbols, files, references…"
                className="h-full min-w-0 flex-1 text-[13.5px]"
                data-testid="global-search"
              />
            )}
          </AnimatePresence>

          <AnimatePresence>
            {expanded && (
              <motion.span
                key="kbd"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.1 }}
                className="kbd flex-none"
              >
                ⌘K
              </motion.span>
            )}
          </AnimatePresence>
        </motion.div>
      </LayoutGroup>

      <SegmentedControl value={view} onChange={setView} />
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
      className="flex-none text-ink-3"
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
