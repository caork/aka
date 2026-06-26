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
import { fetchNodeDetail } from "./repo-api";
import { useAppStore, type ViewId } from "./store";

export default function App() {
  const view = useAppStore((s) => s.view);
  const setView = useAppStore((s) => s.setView);
  const selectedRepoId = useAppStore((s) => s.selectedRepoId);
  const detailTarget = useAppStore((s) => s.detailTarget);
  const openCode = useAppStore((s) => s.openCode);
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

  const changeView = (next: ViewId) => {
    if (next !== "code" || view !== "graph" || !detailTarget) {
      setView(next);
      return;
    }

    const fallbackFile = detailTarget.file;
    if (fallbackFile) {
      openCode({
        repo: selectedRepoId,
        path: fallbackFile,
        line: detailTarget.line > 0 ? detailTarget.line : undefined,
      });
      return;
    }

    setView(next);
    void fetchNodeDetail(selectedRepoId, detailTarget.id).then((res) => {
      const state = useAppStore.getState();
      if (
        state.view !== "code" ||
        state.selectedRepoId !== selectedRepoId ||
        state.detailTarget?.id !== detailTarget.id
      ) {
        return;
      }
      if (res.state !== "ok") return;
      const file = res.detail.file || res.detail.process?.entry?.file;
      const line = res.detail.file ? res.detail.line : (res.detail.process?.entry?.line ?? 0);
      const endLine = res.detail.file ? res.detail.end_line : line;
      if (!file) return;
      openCode({
        repo: selectedRepoId,
        path: file,
        line: line > 0 ? line : undefined,
        endLine: endLine > line ? endLine : undefined,
      });
    });
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
          className="window-drag-region absolute top-0 z-[1]"
          style={{ left: 92, right: 180 }}
          onMouseDown={startWindowDrag}
          data-tauri-drag-region
          aria-hidden
        />

        <div className="absolute bottom-4 left-4 z-20">
          <RepoDropdown />
        </div>

        <div className="absolute right-3 top-3 z-30 flex items-center gap-2">
          <SearchBubble />
          <SegmentedControl value={view} onChange={changeView} />
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
