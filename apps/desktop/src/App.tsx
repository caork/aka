import { AnimatePresence, motion } from "framer-motion";
import CodeWorkspace from "./components/CodeWorkspace";
import DetailPanel from "./components/DetailPanel";
import GraphView from "./components/GraphView";
import SearchBubble from "./components/SearchBubble";
import SegmentedControl from "./components/SegmentedControl";
import Sidebar from "./components/Sidebar";
import { useAppStore } from "./store";

export default function App() {
  const view = useAppStore((s) => s.view);
  const setView = useAppStore((s) => s.setView);

  return (
    <div className="flex h-full">
      <div className="app-backdrop" />
      <Sidebar />
      <main
        className="relative min-h-0 flex-1 overflow-hidden"
        style={{
          background:
            "linear-gradient(135deg, rgba(255,255,255,0.30), rgba(246,250,255,0.18))",
          backdropFilter: "blur(28px) saturate(190%)",
          WebkitBackdropFilter: "blur(28px) saturate(190%)",
          boxShadow:
            "inset 1px 0 0 rgba(255,255,255,0.42), inset 0 0 0 0.5px rgba(15,23,42,0.035)",
        }}
      >
        {/* Search bubble — top-left corner */}
        <div className="absolute left-3 top-3 z-20">
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
