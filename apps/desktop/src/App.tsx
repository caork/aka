import { AnimatePresence, motion } from "framer-motion";
import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import CodeWorkspace from "./components/CodeWorkspace";
import { isDesktopRuntime } from "./desktop-api";
import DetailPanel from "./components/DetailPanel";
import GraphView from "./components/GraphView";
import RepoDropdown from "./components/RepoDropdown";
import SearchBubble from "./components/SearchBubble";
import SegmentedControl from "./components/SegmentedControl";
import { useAppStore } from "./store";

export default function App() {
  const view = useAppStore((s) => s.view);
  const setView = useAppStore((s) => s.setView);
  const syncSystemTheme = useAppStore((s) => s.syncSystemTheme);

  useEffect(() => {
    syncSystemTheme();
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (!media) return;
    media.addEventListener("change", syncSystemTheme);
    return () => media.removeEventListener("change", syncSystemTheme);
  }, [syncSystemTheme]);

  const startWindowDrag = (e: React.MouseEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !isDesktopRuntime()) return;
    e.preventDefault();
    void getCurrentWindow().startDragging().catch(() => {});
  };

  return (
    <div className="flex h-full">
      <div className="app-backdrop" />
      <main
        className="relative min-h-0 flex-1 overflow-hidden"
        style={{
          background: "var(--main-surface)",
          backdropFilter: "blur(24px) saturate(180%)",
          WebkitBackdropFilter: "blur(24px) saturate(180%)",
          boxShadow: "inset 1px 0 0 var(--main-divider)",
        }}
      >
        <div
          className="window-drag-region absolute top-0 z-30"
          style={{ left: 92, right: 180 }}
          onMouseDown={startWindowDrag}
          data-tauri-drag-region
          aria-hidden
        />

        <div className="absolute bottom-4 left-4 z-20">
          <RepoDropdown />
        </div>

        <div className="absolute right-3 top-3 z-20 flex items-center gap-2">
          <SearchBubble />
          <SegmentedControl value={view} onChange={setView} />
        </div>

        <AnimatePresence mode="wait">
          <motion.div
            key={view}
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            transition={{ type: "spring", stiffness: 300, damping: 30 }}
            className="app-content h-full"
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
