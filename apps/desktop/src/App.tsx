import { AnimatePresence, motion } from "framer-motion";
import { useEffect } from "react";
import CodeWorkspace from "./components/CodeWorkspace";
import DetailPanel from "./components/DetailPanel";
import GraphView from "./components/GraphView";
import RepoDropdown from "./components/RepoDropdown";
import SearchBubble from "./components/SearchBubble";
import SegmentedControl from "./components/SegmentedControl";
import { isDesktopRuntime } from "./desktop-api";
import { useAppStore } from "./store";

export default function App() {
  const view = useAppStore((s) => s.view);
  const setView = useAppStore((s) => s.setView);
  const syncSystemTheme = useAppStore((s) => s.syncSystemTheme);
  const needsTitlebarSafeArea =
    isDesktopRuntime() && navigator.userAgent.includes("Mac");

  useEffect(() => {
    syncSystemTheme();
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (!media) return;
    media.addEventListener("change", syncSystemTheme);
    return () => media.removeEventListener("change", syncSystemTheme);
  }, [syncSystemTheme]);

  return (
    <div className="flex h-full">
      <div className="app-backdrop" />
      <main
        className={`relative min-h-0 flex-1 overflow-hidden ${
          needsTitlebarSafeArea ? "titlebar-safe" : ""
        }`}
        style={{
          background: "var(--main-surface)",
          backdropFilter: "blur(24px) saturate(180%)",
          WebkitBackdropFilter: "blur(24px) saturate(180%)",
          boxShadow: "inset 1px 0 0 var(--main-divider)",
        }}
      >
        {/* Logo + Search bubble — top-left, laid out in a row so search expands rightward */}
        <div className="app-top-left-controls absolute top-3 z-20 flex items-center gap-2">
          <RepoDropdown />
          <SearchBubble />
        </div>

        {/* Doc / Graph switcher — top-right corner */}
        <div className="absolute right-3 top-3 z-20">
          <SegmentedControl value={view} onChange={setView} />
        </div>

        <AnimatePresence mode="wait">
          <motion.div
            key={view}
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            transition={{ type: "spring", stiffness: 300, damping: 30 }}
            className="h-full"
          >
            {view === "code" && <CodeWorkspace />}
            {view === "graph" && <GraphView />}
          </motion.div>
        </AnimatePresence>

        <DetailPanel />
      </main>
    </div>
  );
}
