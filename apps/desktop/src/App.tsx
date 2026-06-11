import { AnimatePresence, motion } from "framer-motion";
import DetailPanel from "./components/DetailPanel";
import DocView from "./components/DocView";
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
        className="relative m-3 min-h-0 flex-1 overflow-hidden rounded-[18px]"
        style={{
          background: "rgba(255,255,255,0.38)",
          backdropFilter: "blur(24px) saturate(180%)",
          WebkitBackdropFilter: "blur(24px) saturate(180%)",
          boxShadow:
            "inset 0 0 0 0.5px rgba(15,23,42,0.06), 0 0 0 1px rgba(255,255,255,0.65), 0 2px 6px rgba(16,24,40,.05), 0 16px 40px -12px rgba(16,24,40,.14)",
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
            {view === "doc" && <DocView />}
            {view === "graph" && <GraphView />}
          </motion.div>
        </AnimatePresence>

        <DetailPanel />
      </main>
    </div>
  );
}
