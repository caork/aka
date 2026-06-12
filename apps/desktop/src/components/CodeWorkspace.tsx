import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import CodeView from "./CodeView";
import FileTree from "./FileTree";
import IndexingPanel from "./IndexingPanel";
import SearchPanel from "./SearchPanel";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

const RAIL_WIDTH_KEY = "aka.codeRailWidth";
const DEFAULT_RAIL_WIDTH = 256;
const MIN_RAIL_WIDTH = 220;
const MAX_RAIL_WIDTH = 520;

function clampRailWidth(width: number): number {
  const viewportCap =
    typeof window === "undefined"
      ? MAX_RAIL_WIDTH
      : Math.max(MIN_RAIL_WIDTH, Math.min(MAX_RAIL_WIDTH, window.innerWidth - 360));
  return Math.min(Math.max(width, MIN_RAIL_WIDTH), viewportCap);
}

function readPersistedRailWidth(): number {
  try {
    const saved = Number(localStorage.getItem(RAIL_WIDTH_KEY));
    if (Number.isFinite(saved) && saved > 0) return clampRailWidth(saved);
  } catch {
    /* localStorage may be unavailable in tests/previews. */
  }
  return DEFAULT_RAIL_WIDTH;
}

function persistRailWidth(width: number) {
  try {
    localStorage.setItem(RAIL_WIDTH_KEY, String(Math.round(width)));
  } catch {
    /* ignore persistence failures */
  }
}

/**
 * Code 视图工作区（view === "code"）。
 *   左栏：搜索时为结果列表，否则为文件树（浏览 → 打开文件）
 *   中栏：打开的文件 = GitHub 式全文预览（CodeView，节点定义全部高亮）；
 *         未打开 = 空态提示
 * 右侧连接抽屉（图谱邻居）由 App 的 DetailPanel 叠加渲染。
 */
export default function CodeWorkspace() {
  const codeTarget = useAppStore((s) => s.codeTarget);
  const query = useAppStore((s) => s.query);
  const repos = useAppStore((s) => s.repos);
  const selectedRepoId = useAppStore((s) => s.selectedRepoId);
  const [railWidth, setRailWidth] = useState(readPersistedRailWidth);
  const [resizing, setResizing] = useState(false);
  const dragRef = useRef<{ startX: number; startWidth: number } | null>(null);
  const hasRepos = repos.length > 0;
  const searching = query.trim().length > 0;
  const selectedRepo = repos.find((repo) => repo.id === selectedRepoId) ?? null;
  const showIndexing =
    selectedRepo?.status === "indexing" || selectedRepo?.status === "failed";

  useEffect(() => {
    if (!resizing) return;
    const prevCursor = document.body.style.cursor;
    const prevSelect = document.body.style.userSelect;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    return () => {
      document.body.style.cursor = prevCursor;
      document.body.style.userSelect = prevSelect;
    };
  }, [resizing]);

  useEffect(() => {
    const onResize = () => {
      setRailWidth((width) => {
        const next = clampRailWidth(width);
        if (next !== width) persistRailWidth(next);
        return next;
      });
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  if (repos.length === 0) {
    return (
      <div className="flex h-full items-center justify-center px-6" data-testid="empty-repos">
        <div className="max-w-[340px] text-center">
          <div className="text-[14px] font-semibold text-ink">还没有仓库</div>
          <div className="mt-1 text-[12.5px] leading-relaxed text-ink-3">
            点击左下角 aka 图标里的 Add repository 导入本机目录、Git 仓库或 zip。
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full overflow-hidden">
      {/* 左栏：文件树 / 搜索结果 */}
      <div
        className="themed-border relative flex h-full flex-none flex-col border-r"
        style={{ width: railWidth, minWidth: MIN_RAIL_WIDTH, maxWidth: MAX_RAIL_WIDTH }}
        data-testid="code-rail"
      >
        {searching ? <SearchPanel compact /> : <FileTree />}
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="调整文件浏览栏宽度"
          className="group absolute inset-y-0 right-[-5px] z-30 w-2.5 cursor-col-resize touch-none"
          data-testid="code-rail-resize-handle"
          onPointerDown={(e) => {
            e.preventDefault();
            e.currentTarget.setPointerCapture(e.pointerId);
            dragRef.current = { startX: e.clientX, startWidth: railWidth };
            setResizing(true);
          }}
          onPointerMove={(e) => {
            const d = dragRef.current;
            if (!d) return;
            setRailWidth(clampRailWidth(d.startWidth + (e.clientX - d.startX)));
          }}
          onPointerUp={(e) => {
            const d = dragRef.current;
            if (!d) return;
            dragRef.current = null;
            setResizing(false);
            const final = clampRailWidth(d.startWidth + (e.clientX - d.startX));
            setRailWidth(final);
            persistRailWidth(final);
          }}
          onPointerCancel={() => {
            dragRef.current = null;
            setResizing(false);
          }}
        >
          <div
            className="absolute bottom-2 right-[3px] top-2 w-[3px] rounded-full bg-transparent transition-colors duration-150 ease-out group-hover:bg-[rgba(46,124,246,0.4)]"
            style={resizing ? { background: "rgba(46,124,246,0.55)" } : undefined}
          />
        </div>
      </div>

      {/* 中栏：代码 / 空态 */}
      <div className="relative min-w-0 flex-1">
        <AnimatePresence mode="wait">
          {showIndexing && selectedRepo ? (
            <motion.div
              key={`indexing ${selectedRepo.id} ${selectedRepo.status}`}
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0 }}
              transition={spring}
              className="h-full"
            >
              <IndexingPanel repo={selectedRepo} />
            </motion.div>
          ) : codeTarget ? (
            <motion.div
              key={`${codeTarget.repo} ${codeTarget.path}`}
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0 }}
              transition={spring}
              className="h-full"
            >
              <CodeView />
            </motion.div>
          ) : (
            <motion.div
              key="empty"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={spring}
              className="flex h-full items-center justify-center"
              data-testid="code-landing"
            >
              <span className="text-[12px] text-ink-3">
                {hasRepos ? "请点击左侧文件开始浏览" : "请先导入一个代码仓库"}
              </span>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}
