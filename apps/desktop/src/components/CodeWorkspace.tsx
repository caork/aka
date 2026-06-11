import { AnimatePresence, motion } from "framer-motion";
import CodeView from "./CodeView";
import FileTree from "./FileTree";
import SearchPanel from "./SearchPanel";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

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
  const hasRepos = useAppStore((s) => s.repos.length > 0);
  const searching = query.trim().length > 0;

  return (
    <div className="flex h-full overflow-hidden">
      {/* 左栏：文件树 / 搜索结果 */}
      <div
        className="themed-border flex h-full w-[256px] flex-none flex-col border-r"
        data-testid="code-rail"
      >
        {searching ? <SearchPanel compact /> : <FileTree />}
      </div>

      {/* 中栏：代码 / 空态 */}
      <div className="relative min-w-0 flex-1">
        <AnimatePresence mode="wait">
          {codeTarget ? (
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
