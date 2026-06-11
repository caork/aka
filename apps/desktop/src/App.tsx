import { AnimatePresence, motion } from "framer-motion";
import CommandBar from "./components/CommandBar";
import DetailPanel from "./components/DetailPanel";
import DocView from "./components/DocView";
import GraphView from "./components/GraphView";
import Sidebar from "./components/Sidebar";
import { useAppStore } from "./store";

export default function App() {
  const view = useAppStore((s) => s.view);

  return (
    <div className="flex h-full">
      <div className="app-backdrop" />
      <Sidebar />
      <div className="flex min-w-0 flex-1 flex-col">
        <CommandBar />
        <main className="relative m-3 min-h-0 flex-1 overflow-hidden rounded-[18px]">
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
    </div>
  );
}
