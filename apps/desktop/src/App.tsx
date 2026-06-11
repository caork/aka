import { AnimatePresence, motion } from "framer-motion";
import CodeWorkspace from "./components/CodeWorkspace";
import DetailPanel from "./components/DetailPanel";
import GraphView from "./components/GraphView";
import RepoDropdown from "./components/RepoDropdown";
import SearchBubble from "./components/SearchBubble";
import SegmentedControl from "./components/SegmentedControl";
import { useAppStore } from "./store";

export default function App() {
  const view = useAppStore((s) => s.view);
  const setView = useAppStore((s) => s.setView);

  return (
    <div className="flex h-full">
      <div className="app-backdrop" />
      <main
        className="relative min-h-0 flex-1 overflow-hidden"
        style={{
          background: "rgba(255,255,255,0.38)",
          backdropFilter: "blur(24px) saturate(180%)",
          WebkitBackdropFilter: "blur(24px) saturate(180%)",
        }}
      >
        {/* Logo + Search bubble — top-left, laid out in a row so search expands rightward */}
        <div className="absolute left-3 top-3 z-20 flex items-center gap-2">
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
