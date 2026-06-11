import { useEffect, useMemo, useRef, useState } from "react";
import { mockFiles } from "../mock";
import { fetchRepoFiles, type RepoFile } from "../repo-api";
import { useAppStore } from "../store";

/* ============================== 树模型 ============================== */

interface DirNode {
  type: "dir";
  /** 折叠链后的显示名（可能含 "/"，如 aka-core/src） */
  name: string;
  /** 完整相对路径（用作展开态 key 与稳定 id） */
  path: string;
  children: TreeNode[];
}
interface FileNode {
  type: "file";
  name: string;
  path: string;
  symbols: number;
}
type TreeNode = DirNode | FileNode;

interface RawDir {
  dirs: Map<string, RawDir>;
  files: { name: string; path: string; symbols: number }[];
}

/** 扁平路径 → 嵌套树，并折叠单子目录链（GitHub 式 a/b/c 合并成一行）。 */
function buildTree(files: RepoFile[]): TreeNode[] {
  const root: RawDir = { dirs: new Map(), files: [] };
  for (const f of files) {
    const parts = f.path.split("/").filter(Boolean);
    let cur = root;
    for (let i = 0; i < parts.length - 1; i++) {
      const seg = parts[i];
      let next = cur.dirs.get(seg);
      if (!next) {
        next = { dirs: new Map(), files: [] };
        cur.dirs.set(seg, next);
      }
      cur = next;
    }
    cur.files.push({
      name: parts[parts.length - 1] ?? f.path,
      path: f.path,
      symbols: f.symbols,
    });
  }

  const toNodes = (dir: RawDir, prefix: string): TreeNode[] => {
    const dirNodes: DirNode[] = [];
    for (const [seg, sub] of dir.dirs) {
      let name = seg;
      let path = prefix ? `${prefix}/${seg}` : seg;
      let node = sub;
      /* 折叠链：唯一子目录且无文件 → 合并显示 */
      while (node.dirs.size === 1 && node.files.length === 0) {
        const [childSeg, childDir] = node.dirs.entries().next().value as [
          string,
          RawDir,
        ];
        name = `${name}/${childSeg}`;
        path = `${path}/${childSeg}`;
        node = childDir;
      }
      dirNodes.push({
        type: "dir",
        name,
        path,
        children: toNodes(node, path),
      });
    }
    dirNodes.sort((a, b) => a.name.localeCompare(b.name));
    const fileNodes: FileNode[] = dir.files
      .map((f) => ({ type: "file" as const, ...f }))
      .sort((a, b) => a.name.localeCompare(b.name));
    return [...dirNodes, ...fileNodes];
  };

  return toNodes(root, "");
}

/** 活动文件的所有祖先目录路径（用于自动展开）。 */
function ancestorDirs(nodes: TreeNode[], filePath: string): string[] {
  const out: string[] = [];
  const walk = (list: TreeNode[]): boolean => {
    for (const n of list) {
      if (n.type === "file") {
        if (n.path === filePath) return true;
      } else if (filePath === n.path || filePath.startsWith(n.path + "/")) {
        out.push(n.path);
        walk(n.children);
        return true;
      }
    }
    return false;
  };
  walk(nodes);
  return out;
}

type LoadState =
  | { phase: "loading" }
  | { phase: "ok"; files: RepoFile[]; live: boolean }
  /** serve 可达但端点不支持/仓库未找到——不假装有文件 */
  | { phase: "unsupported" };

/* ============================== 组件 ============================== */

export default function FileTree() {
  const repoId = useAppStore((s) => s.selectedRepoId);
  const repos = useAppStore((s) => s.repos);
  const codeTarget = useAppStore((s) => s.codeTarget);
  const openCode = useAppStore((s) => s.openCode);

  const repoName = repos.find((r) => r.id === repoId)?.name ?? repoId;
  const activePath = codeTarget?.path ?? null;

  const [load, setLoad] = useState<LoadState>({ phase: "loading" });
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  /* 已对当前文件做过自动展开，避免覆盖用户手动折叠 */
  const autoFor = useRef<string | null>(null);

  useEffect(() => {
    let stale = false;
    const ctrl = new AbortController();
    setLoad({ phase: "loading" });
    void fetchRepoFiles(repoId, ctrl.signal)
      .then((res) => {
        if (stale) return;
        if (res.state === "ok") {
          /* 真实数据（含空仓库 → 走"暂无文件"空态，不造假） */
          setLoad({ phase: "ok", files: res.files, live: true });
        } else if (res.state === "offline") {
          /* 仅在 serve 真正不可达时回退演示数据（离线展示） */
          setLoad({ phase: "ok", files: mockFiles(), live: false });
        } else {
          /* serve 可达但 404/501 —— 给明确提示，不展示会死链的假文件 */
          setLoad({ phase: "unsupported" });
        }
      })
      .catch(() => {
        /* AbortError —— 忽略 */
      });
    return () => {
      stale = true;
      ctrl.abort();
    };
  }, [repoId]);

  const tree = useMemo(
    () => (load.phase === "ok" ? buildTree(load.files) : []),
    [load],
  );

  /* 首次加载默认展开第一层目录 */
  useEffect(() => {
    if (load.phase !== "ok") return;
    setExpanded((prev) => {
      if (prev.size > 0) return prev;
      const next = new Set(prev);
      for (const n of tree) if (n.type === "dir") next.add(n.path);
      return next;
    });
  }, [load.phase, tree]);

  /* 活动文件变化时展开其祖先链 */
  useEffect(() => {
    if (!activePath || load.phase !== "ok") return;
    if (autoFor.current === activePath) return;
    autoFor.current = activePath;
    const anc = ancestorDirs(tree, activePath);
    if (anc.length === 0) return;
    setExpanded((prev) => {
      const next = new Set(prev);
      for (const p of anc) next.add(p);
      return next;
    });
  }, [activePath, tree, load.phase]);

  const toggle = (path: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const total = load.phase === "ok" ? load.files.length : 0;

  return (
    <div className="flex h-full flex-col" data-testid="file-tree">
      {/* 顶部留白避开浮动搜索泡 */}
      <div className="flex items-baseline justify-between px-3 pb-2 pt-14">
        <span className="text-[10.5px] font-semibold uppercase tracking-[0.08em] text-ink-3">
          Files
        </span>
        {total > 0 && (
          <span className="tabular text-[10.5px] text-ink-3">{total}</span>
        )}
      </div>

      <div className="scroll-area min-h-0 flex-1 px-1.5 pb-3">
        {load.phase === "loading" && <TreeSkeleton />}
        {load.phase === "unsupported" && (
          <div className="px-3 py-6 text-center text-[12px] leading-relaxed text-ink-3">
            该仓库未在 aka serve 注册,或后端版本过旧
            <br />
            <span className="text-[11px]">选择左侧已索引的仓库</span>
          </div>
        )}
        {load.phase === "ok" && load.files.length === 0 && (
          <div className="px-3 py-6 text-center text-[12px] text-ink-3">
            该仓库暂无可浏览的文件
          </div>
        )}
        {load.phase === "ok" &&
          tree.map((node) => (
            <TreeRow
              key={node.path}
              node={node}
              depth={0}
              expanded={expanded}
              activePath={activePath}
              onToggle={toggle}
              onOpenFile={(path) => openCode({ repo: repoName, path })}
            />
          ))}
      </div>

      {load.phase === "ok" && !load.live && (
        <div className="border-t border-[rgba(15,23,42,0.06)] px-3 py-1.5 text-[10px] text-ink-3">
          演示文件树 · 连接 aka serve 后显示真实数据
        </div>
      )}
    </div>
  );
}

/* ============================== 行 ============================== */

function TreeRow({
  node,
  depth,
  expanded,
  activePath,
  onToggle,
  onOpenFile,
}: {
  node: TreeNode;
  depth: number;
  expanded: Set<string>;
  activePath: string | null;
  onToggle(path: string): void;
  onOpenFile(path: string): void;
}) {
  const pad = 8 + depth * 13;

  if (node.type === "dir") {
    const open = expanded.has(node.path);
    return (
      <div>
        <button
          onClick={() => onToggle(node.path)}
          className="focus-ring group flex w-full items-center gap-1 rounded-[7px] py-[5px] pr-2 text-left transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.045)]"
          style={{ paddingLeft: pad }}
          data-testid="tree-dir"
        >
          <Chevron open={open} />
          <FolderIcon open={open} />
          <span className="min-w-0 flex-1 truncate text-[12.5px] font-medium text-ink-2">
            {node.name}
          </span>
        </button>
        {open &&
          node.children.map((child) => (
            <TreeRow
              key={child.path}
              node={child}
              depth={depth + 1}
              expanded={expanded}
              activePath={activePath}
              onToggle={onToggle}
              onOpenFile={onOpenFile}
            />
          ))}
      </div>
    );
  }

  const active = node.path === activePath;
  return (
    <button
      onClick={() => onOpenFile(node.path)}
      className="focus-ring group flex w-full items-center gap-1.5 rounded-[7px] py-[5px] pr-2 text-left transition-colors duration-150 ease-out"
      style={{
        paddingLeft: pad + 4,
        background: active ? "rgba(46,124,246,0.09)" : undefined,
      }}
      data-testid="tree-file"
      aria-current={active ? "true" : undefined}
      title={node.path}
    >
      <FileIcon />
      <span
        className={`min-w-0 flex-1 truncate text-[12.5px] ${
          active ? "font-semibold text-[#2e7cf6]" : "text-ink"
        }`}
      >
        {node.name}
      </span>
      {node.symbols > 0 && (
        <span className="tabular flex-none text-[10px] text-ink-3">
          {node.symbols}
        </span>
      )}
    </button>
  );
}

/* ============================== 图标 / 骨架 ============================== */

function Chevron({ open }: { open: boolean }) {
  return (
    <svg
      width="11"
      height="11"
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
      className="flex-none text-ink-3 transition-transform duration-150 ease-out"
      style={{ transform: open ? "rotate(90deg)" : "rotate(0deg)" }}
    >
      <path
        d="m9 6 6 6-6 6"
        stroke="currentColor"
        strokeWidth="2.2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function FolderIcon({ open }: { open: boolean }) {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
      className="flex-none"
      style={{ color: open ? "#2e7cf6" : "#94a3b8" }}
    >
      <path
        d="M3 7a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinejoin="round"
        fill={open ? "rgba(46,124,246,0.08)" : "none"}
      />
    </svg>
  );
}

function FileIcon() {
  return (
    <svg
      width="13"
      height="13"
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
      className="flex-none text-ink-3 opacity-70"
    >
      <path
        d="M14 3H7a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V7l-4-4Z"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinejoin="round"
      />
      <path d="M14 3v4h4" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round" />
    </svg>
  );
}

function TreeSkeleton() {
  const rows = [
    { w: "64%", d: 0 },
    { w: "48%", d: 1 },
    { w: "70%", d: 1 },
    { w: "56%", d: 2 },
    { w: "62%", d: 2 },
    { w: "44%", d: 1 },
    { w: "68%", d: 0 },
    { w: "52%", d: 1 },
  ];
  return (
    <div className="space-y-2 px-2 py-2">
      {rows.map((r, i) => (
        <div
          key={i}
          className="h-[11px] animate-pulse rounded-[4px]"
          style={{
            width: r.w,
            marginLeft: r.d * 13,
            background: "rgba(15,23,42,0.06)",
          }}
        />
      ))}
    </div>
  );
}
