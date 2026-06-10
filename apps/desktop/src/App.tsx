import { AnimatePresence, motion } from "framer-motion";
import CodeView from "./components/CodeView";
import CommandBar from "./components/CommandBar";
import DetailPanel from "./components/DetailPanel";
import GraphView from "./components/GraphView";
import SearchView from "./components/SearchView";
import Sidebar from "./components/Sidebar";
import SymbolView from "./components/SymbolView";
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
              {view === "search" && <SearchView />}
              {view === "graph" && <GraphView />}
              {view === "symbol" && <SymbolView />}
              {view === "code" && <CodeView />}
            </motion.div>
          </AnimatePresence>
          {/* 三视图共用的右侧详情面板（在 main 内，不遮 toolbar/侧栏） */}
          <DetailPanel />
        </main>
      </div>
    </div>
  );
}
