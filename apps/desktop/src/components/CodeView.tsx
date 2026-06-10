import { AnimatePresence, motion } from "framer-motion";
import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { hashComments, renderTokens } from "../code/highlight";
import {
  fetchFileSymbols,
  fetchSource,
  type FileSymbol,
} from "../repo-api";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

/** /api/source 单次最多返回的行数（后端合同） */
const CHUNK = 2000;
/** 打开时为覆盖目标行最多自动连续拉取的块数（25 × 2000 = 5 万行） */
const MAX_AUTO_CHUNKS = 25;

const POPOVER_W = 280;
const POPOVER_H = 170;

type LoadState =
  | { phase: "loading" }
  | { phase: "error"; kind: "unsupported" | "binary" | "offline" }
  | {
      phase: "ok";
      lines: string[];
      totalLines: number;
      absPath: string;
      /** 还有未加载的尾部 */
      truncated: boolean;
    };

interface LineHighlight {
  start: number;
  end: number;
}

interface Popover {
  sym: FileSymbol;
  x: number;
  y: number;
}

const LANG_BY_EXT: Record<string, string> = {
  ts: "TypeScript",
  tsx: "TSX",
  js: "JavaScript",
  jsx: "JSX",
  mjs: "JavaScript",
  rs: "Rust",
  py: "Python",
  go: "Go",
  java: "Java",
  kt: "Kotlin",
  c: "C",
  h: "C",
  cc: "C++",
  cpp: "C++",
  hpp: "C++",
  cs: "C#",
  rb: "Ruby",
  php: "PHP",
  swift: "Swift",
  scala: "Scala",
  sh: "Shell",
  bash: "Shell",
  zsh: "Shell",
  css: "CSS",
  html: "HTML",
  json: "JSON",
  toml: "TOML",
  yml: "YAML",
  yaml: "YAML",
  md: "Markdown",
  sql: "SQL",
  vue: "Vue",
  lua: "Lua",
};

/**
 * GitHub 式全文源码预览（view === "code"）。
 *
 * 数据两路并发，均按离线/旧后端优雅降级：
 *   GET /api/source        → 全文（2000 行/块，truncated 时可继续加载）
 *   GET /api/file/symbols  → 文件内符号 → 行内可点 span → graph 联动浮窗
 */
export default function CodeView() {
  const target = useAppStore((s) => s.codeTarget);
  if (!target) return null;
  return (
    <CodeBody
      key={`${target.repo}\u0000${target.path}`}
      repo={target.repo}
      path={target.path}
    />
  );
}

function CodeBody({ repo, path }: { repo: string; path: string }) {
  /* line/endLine 跟随 codeTarget 变化（同文件重新定位不重载） */
  const target = useAppStore((s) => s.codeTarget);
  const openDetail = useAppStore((s) => s.openDetail);
  const requestEgo = useAppStore((s) => s.requestEgo);
  const requestFocus = useAppStore((s) => s.requestFocus);

  const [load, setLoad] = useState<LoadState>({ phase: "loading" });
  const [symbols, setSymbols] = useState<FileSymbol[]>([]);
  const [highlight, setHighlight] = useState<LineHighlight | null>(null);
  const [pendingScroll, setPendingScroll] = useState<number | null>(null);
  const [popover, setPopover] = useState<Popover | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [copied, setCopied] = useState(false);

  const scrollRef = useRef<HTMLDivElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);

  /* ---- 全文加载（首块起步，自动续拉直到覆盖目标行） ---- */
  useEffect(() => {
    let stale = false;
    const ctrl = new AbortController();
    setLoad({ phase: "loading" });
    setSymbols([]);
    setPopover(null);
    const wantLine = useAppStore.getState().codeTarget?.line ?? 1;

    const run = async () => {
      const all: string[] = [];
      let total = 0;
      let absPath = "";
      let truncated = false;
      try {
        for (let chunk = 0; chunk < MAX_AUTO_CHUNKS; chunk++) {
          const res = await fetchSource(
            repo,
            path,
            all.length + 1,
            all.length + CHUNK,
            ctrl.signal,
          );
          if (stale) return;
          if (res.state !== "ok") {
            if (all.length === 0) {
              setLoad({ phase: "error", kind: res.state });
            } else {
              /* 中途失败——保留已加载部分，可手动重试「加载更多」 */
              setLoad({
                phase: "ok",
                lines: all,
                totalLines: total,
                absPath,
                truncated: true,
              });
            }
            return;
          }
          for (const ln of res.source.lines) all.push(ln);
          total = res.source.total_lines;
          absPath = res.source.abs_path;
          truncated = res.source.truncated || all.length < total;
          if (!truncated || all.length >= wantLine) break;
        }
      } catch {
        /* AbortError（目标已切换）——丢弃 */
        return;
      }
      if (!stale) {
        setLoad({ phase: "ok", lines: all, totalLines: total, absPath, truncated });
      }
    };
    void run();

    void fetchFileSymbols(repo, path, ctrl.signal)
      .then((res) => {
        if (!stale && res.state === "ok") setSymbols(res.symbols);
        /* unsupported / offline → 无符号联动，纯文本预览 */
      })
      .catch(() => {
        /* AbortError——忽略 */
      });

    return () => {
      stale = true;
      ctrl.abort();
    };
  }, [repo, path]);

  /* ---- codeTarget 变化（含同文件重新定位）：重置高亮 + 待滚动行 ---- */
  useEffect(() => {
    if (!target) return;
    const start = target.line ?? 0;
    setHighlight(
      start > 0
        ? { start, end: Math.max(target.endLine ?? start, start) }
        : null,
    );
    setPendingScroll(start > 0 ? start : null);
  }, [target]);

  /* ---- 「加载更多」：追加下一段（start = 已载末行 + 1） ---- */
  const loadMore = useCallback(async () => {
    if (load.phase !== "ok" || loadingMore) return;
    const from = load.lines.length;
    setLoadingMore(true);
    try {
      const res = await fetchSource(repo, path, from + 1, from + CHUNK);
      if (res.state !== "ok") return;
      setLoad((prev) => {
        if (prev.phase !== "ok" || prev.lines.length !== from) return prev;
        const lines = prev.lines.concat(res.source.lines);
        return {
          phase: "ok",
          lines,
          totalLines: res.source.total_lines,
          absPath: res.source.abs_path || prev.absPath,
          truncated: res.source.truncated || lines.length < res.source.total_lines,
        };
      });
    } finally {
      setLoadingMore(false);
    }
  }, [load, loadingMore, repo, path]);

  /* ---- 滚动定位：目标行已加载就居中，未加载则继续自动拉取 ---- */
  useEffect(() => {
    if (pendingScroll === null || load.phase !== "ok") return;
    if (pendingScroll > load.totalLines && !load.truncated) {
      setPendingScroll(null);
      return;
    }
    if (pendingScroll <= load.lines.length) {
      const el = scrollRef.current?.querySelector(
        `[data-ln="${pendingScroll}"]`,
      );
      el?.scrollIntoView({ block: "center" });
      setPendingScroll(null);
    } else if (load.truncated && !loadingMore) {
      void loadMore();
    } else if (!load.truncated) {
      setPendingScroll(null);
    }
  }, [pendingScroll, load, loadingMore, loadMore]);

  /* ---- 符号浮窗：Esc / 点外关闭 ---- */
  useEffect(() => {
    if (!popover) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setPopover(null);
    };
    const onDown = (e: PointerEvent) => {
      if (e.target instanceof Element && popoverRef.current?.contains(e.target))
        return;
      setPopover(null);
    };
    window.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onDown);
    };
  }, [popover]);

  /* ---- 派生数据 ---- */
  const hash = useMemo(() => hashComments(path), [path]);
  const symbolsByLine = useMemo(() => {
    const m = new Map<number, FileSymbol[]>();
    for (const s of symbols) {
      if (!s.name || s.line <= 0) continue;
      const arr = m.get(s.line);
      if (arr) arr.push(s);
      else m.set(s.line, [s]);
    }
    return m;
  }, [symbols]);

  const onGutterClick = useCallback((ln: number) => {
    setHighlight({ start: ln, end: ln });
  }, []);

  const onSymbolClick = useCallback((sym: FileSymbol, el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    setPopover({
      sym,
      x: Math.max(8, Math.min(r.left, window.innerWidth - POPOVER_W - 12)),
      y: Math.min(r.bottom + 8, window.innerHeight - POPOVER_H - 12),
    });
  }, []);

  const absPath = load.phase === "ok" ? load.absPath : "";
  const totalLines = load.phase === "ok" ? load.totalLines : 0;
  const focusLine = highlight?.start ?? target?.line ?? 0;
  const editorHref = absPath
    ? `vscode://file/${encodeURI(absPath)}${focusLine > 0 ? `:${focusLine}` : ""}`
    : null;
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  const lang = LANG_BY_EXT[ext] ?? (ext ? ext.toUpperCase() : null);

  const copyPath = () => {
    const text = absPath || path;
    void navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1600);
      })
      .catch(() => {
        /* 剪贴板不可用——静默忽略 */
      });
  };

  const segments = path.split("/").filter(Boolean);
  const gutterWidth = `${Math.max(4, String(Math.max(totalLines, load.phase === "ok" ? load.lines.length : 0)).length) + 2}ch`;

  return (
    <div
      className="glass-panel flex h-full flex-col overflow-hidden"
      data-testid="code-view"
    >
      {/* ---- 头部：面包屑 + 徽章 + 操作 ---- */}
      <div className="flex flex-none items-center gap-3 border-b border-[rgba(15,23,42,0.06)] px-4 py-2.5">
        <nav
          className="mono flex min-w-0 flex-1 items-center gap-1 text-[12px]"
          aria-label="文件路径"
          title={`${repo} / ${path}`}
        >
          <span className="flex-none font-semibold text-ink-2">{repo}</span>
          <span className="flex-none text-ink-3" aria-hidden>
            /
          </span>
          {segments.length > 1 && (
            <>
              {/* 目录部分整体可截断，文件名永远完整可见 */}
              <span className="min-w-[3ch] truncate text-ink-2">
                {segments.slice(0, -1).join("/")}
              </span>
              <span className="flex-none text-ink-3" aria-hidden>
                /
              </span>
            </>
          )}
          <span className="max-w-[40%] flex-none truncate font-semibold text-ink">
            {segments[segments.length - 1] ?? path}
          </span>
        </nav>

        {totalLines > 0 && (
          <span
            className="tabular flex-none rounded-[6px] px-1.5 py-0.5 text-[10.5px] font-semibold text-ink-2"
            style={{ background: "rgba(15,23,42,0.05)" }}
            data-testid="code-total-lines"
          >
            {totalLines.toLocaleString()} 行
          </span>
        )}
        {lang && (
          <span
            className="flex-none rounded-[6px] px-1.5 py-0.5 text-[10.5px] font-semibold uppercase tracking-wide"
            style={{ color: "#2563c9", background: "rgba(46,124,246,0.08)" }}
          >
            {lang}
          </span>
        )}

        <span className="h-4 w-px flex-none bg-[rgba(15,23,42,0.08)]" aria-hidden />

        {editorHref ? (
          <a
            href={editorHref}
            className="focus-ring flex-none rounded-[8px] px-2.5 py-1.5 text-[12px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)] hover:text-ink"
            style={{ boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.1)" }}
            data-testid="code-open-editor"
          >
            在编辑器打开
          </a>
        ) : (
          <span
            className="flex-none cursor-not-allowed rounded-[8px] px-2.5 py-1.5 text-[12px] font-medium text-ink-3 opacity-60"
            style={{ boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.08)" }}
            title="源码绝对路径不可用（需 aka serve 在线）"
            data-testid="code-open-editor"
          >
            在编辑器打开
          </span>
        )}
        <button
          onClick={copyPath}
          className="focus-ring flex-none rounded-[8px] px-2.5 py-1.5 text-[12px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)] hover:text-ink"
          style={{ boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.1)" }}
          data-testid="code-copy-path"
        >
          {copied ? "已复制 ✓" : "复制路径"}
        </button>
      </div>

      {/* ---- 代码区 ---- */}
      {load.phase === "loading" && <CodeSkeleton />}
      {load.phase === "error" && <CodeEmpty kind={load.kind} path={path} />}
      {load.phase === "ok" && (
        <div
          ref={scrollRef}
          onScroll={() => setPopover(null)}
          className="scroll-area min-h-0 flex-1 overflow-x-auto"
          style={{ background: "#fbfcfe" }}
          data-testid="code-scroll"
        >
          <div className="w-max min-w-full pb-6 pt-1.5">
            {load.lines.map((text, i) => {
              const ln = i + 1;
              return (
                <CodeLine
                  key={ln}
                  ln={ln}
                  text={text}
                  hash={hash}
                  gutterWidth={gutterWidth}
                  focused={
                    highlight !== null &&
                    ln >= highlight.start &&
                    ln <= highlight.end
                  }
                  syms={symbolsByLine.get(ln)}
                  onGutterClick={onGutterClick}
                  onSymbolClick={onSymbolClick}
                />
              );
            })}

            {load.truncated && (
              <div className="sticky left-0 flex items-center gap-3 px-4 py-3">
                <button
                  onClick={() => void loadMore()}
                  disabled={loadingMore}
                  className="focus-ring rounded-[9px] px-3 py-1.5 text-[12px] font-medium text-[#2563c9] transition-colors duration-150 ease-out hover:bg-[rgba(46,124,246,0.08)] disabled:cursor-not-allowed disabled:opacity-50"
                  style={{ boxShadow: "inset 0 0 0 0.5px rgba(46,124,246,0.3)" }}
                  data-testid="code-load-more"
                >
                  {loadingMore ? "加载中…" : "加载更多"}
                </button>
                <span className="tabular text-[11.5px] text-ink-3">
                  已加载 {load.lines.length.toLocaleString()} /{" "}
                  {load.totalLines.toLocaleString()} 行
                </span>
              </div>
            )}
          </div>
        </div>
      )}

      {/* ---- 符号浮窗（anchored popover） ---- */}
      <AnimatePresence>
        {popover && (
          <motion.div
            ref={popoverRef}
            initial={{ opacity: 0, y: 6, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 4, scale: 0.98 }}
            transition={spring}
            className="glass-panel fixed z-50 p-3"
            style={{ left: popover.x, top: popover.y, width: POPOVER_W }}
            data-testid="symbol-popover"
            role="dialog"
            aria-label={`${popover.sym.name} 符号操作`}
          >
            <div className="flex items-center gap-2">
              <span className="mono min-w-0 truncate text-[13px] font-semibold text-ink">
                {popover.sym.name}
              </span>
              <span className={`badge ${popover.sym.label}`}>
                {popover.sym.label}
              </span>
            </div>
            <div className="mono tabular mt-1 text-[11px] text-ink-3">
              L{popover.sym.line}
              {popover.sym.end_line > popover.sym.line
                ? `–${popover.sym.end_line}`
                : ""}
            </div>
            <div className="mt-2.5 grid grid-cols-1 gap-1.5">
              <button
                onClick={() => {
                  setPopover(null);
                  requestFocus(popover.sym.id, popover.sym.name);
                }}
                className="btn-primary focus-ring px-3 py-1.5 text-[12px] font-semibold"
                data-testid="popover-focus-graph"
              >
                在 Graph 中定位
              </button>
              <div className="grid grid-cols-2 gap-1.5">
                <button
                  onClick={() => {
                    setPopover(null);
                    requestEgo(popover.sym.id, popover.sym.name);
                  }}
                  className="focus-ring rounded-[9px] px-3 py-1.5 text-[12px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)] hover:text-ink"
                  style={{ boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.1)" }}
                  data-testid="popover-ego"
                >
                  Ego 视图
                </button>
                <button
                  onClick={() => {
                    setPopover(null);
                    openDetail({
                      id: popover.sym.id,
                      name: popover.sym.name,
                      label: popover.sym.label,
                      file: popover.sym.file,
                      line: popover.sym.line,
                    });
                  }}
                  className="focus-ring rounded-[9px] px-3 py-1.5 text-[12px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)] hover:text-ink"
                  style={{ boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.1)" }}
                  data-testid="popover-detail"
                >
                  符号详情
                </button>
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/* ============================== 行渲染 ============================== */

const CodeLine = memo(function CodeLine({
  ln,
  text,
  hash,
  gutterWidth,
  focused,
  syms,
  onGutterClick,
  onSymbolClick,
}: {
  ln: number;
  text: string;
  hash: boolean;
  gutterWidth: string;
  focused: boolean;
  syms: FileSymbol[] | undefined;
  onGutterClick(ln: number): void;
  onSymbolClick(sym: FileSymbol, el: HTMLElement): void;
}) {
  return (
    <div
      data-ln={ln}
      className="mono flex w-max min-w-full text-[12px] leading-[1.7]"
      style={focused ? { background: "rgba(46,124,246,0.07)" } : undefined}
    >
      {/* 行号栏：sticky left，横向滚动时钉住；不透明背景避免重叠透字 */}
      <button
        onClick={() => onGutterClick(ln)}
        aria-label={`高亮第 ${ln} 行`}
        className="tabular sticky left-0 z-[1] flex-none cursor-pointer select-none pr-3 text-right text-ink-3 transition-colors duration-150 ease-out hover:text-[#2563c9]"
        style={{
          font: "inherit",
          width: gutterWidth,
          background: focused ? "#e9f1fe" : "#f4f6fa",
          boxShadow: focused
            ? "inset 2.5px 0 0 rgba(46,124,246,0.65), inset -0.5px 0 0 rgba(15,23,42,0.06)"
            : "inset -0.5px 0 0 rgba(15,23,42,0.06)",
        }}
      >
        {ln}
      </button>
      {/* 与行号留足间距（pl-4），white-space: pre 不换行 */}
      <span className="whitespace-pre pl-4 pr-8 text-ink-2">
        {renderLine(text, hash, syms, onSymbolClick)}
        {text.length === 0 ? " " : null}
      </span>
    </div>
  );
});

/** 行内符号注入：该行是符号起始行时，把 name 的首次完整词匹配包成可点 span。 */
function renderLine(
  text: string,
  hash: boolean,
  syms: FileSymbol[] | undefined,
  onSymbolClick: (sym: FileSymbol, el: HTMLElement) => void,
): ReactNode {
  if (syms) {
    for (const sym of syms) {
      const idx = findWholeWord(text, sym.name);
      if (idx < 0) continue;
      const pre = text.slice(0, idx);
      const mid = text.slice(idx, idx + sym.name.length);
      const post = text.slice(idx + sym.name.length);
      return (
        <>
          {renderTokens(pre, hash)}
          <button
            onClick={(e) => onSymbolClick(sym, e.currentTarget)}
            title={`${sym.label} · 联动 Graph`}
            className="focus-ring cursor-pointer rounded-[3px] border-b border-dashed border-[rgba(46,124,246,0.5)] bg-[rgba(46,124,246,0.07)] align-baseline font-medium text-[#2563c9] transition-colors duration-150 ease-out hover:border-[rgba(46,124,246,0.8)] hover:bg-[rgba(46,124,246,0.16)]"
            style={{ font: "inherit", padding: "0 1px" }}
            data-testid="code-symbol"
          >
            {mid}
          </button>
          {renderTokens(post, hash)}
        </>
      );
    }
  }
  return renderTokens(text, hash);
}

function isWordChar(c: string): boolean {
  return /[\w$]/.test(c);
}

/** name 在 text 中的首次完整词匹配位置（前后非标识符字符），无则 -1。 */
function findWholeWord(text: string, name: string): number {
  if (!name) return -1;
  let from = 0;
  for (;;) {
    const i = text.indexOf(name, from);
    if (i < 0) return -1;
    const before = i > 0 ? text[i - 1] : "";
    const after = i + name.length < text.length ? text[i + name.length] : "";
    if (
      (before === "" || !isWordChar(before)) &&
      (after === "" || !isWordChar(after))
    ) {
      return i;
    }
    from = i + 1;
  }
}

/* ============================== 空态 / 骨架 ============================== */

function CodeSkeleton() {
  const widths = ["62%", "88%", "47%", "78%", "94%", "55%", "70%", "84%", "40%", "66%"];
  return (
    <div
      className="min-h-0 flex-1 px-5 py-4"
      style={{ background: "#fbfcfe" }}
      data-testid="code-skeleton"
    >
      <div className="space-y-2.5">
        {widths.map((w, i) => (
          <div
            key={i}
            className="h-[11px] animate-pulse rounded-[4px]"
            style={{ width: w, background: "rgba(15,23,42,0.06)" }}
          />
        ))}
      </div>
    </div>
  );
}

function CodeEmpty({
  kind,
  path,
}: {
  kind: "unsupported" | "binary" | "offline";
  path: string;
}) {
  const message =
    kind === "binary"
      ? "非文本文件，无法预览"
      : kind === "unsupported"
        ? "当前后端不支持源码预览（需更新 aka serve）"
        : "无法连接本地 aka serve（127.0.0.1:4111）";
  return (
    <div
      className="flex min-h-0 flex-1 items-center justify-center"
      style={{ background: "#fbfcfe" }}
    >
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={spring}
        className="glass flex max-w-[420px] flex-col items-center gap-1.5 px-8 py-10 text-center"
        data-testid="code-empty"
      >
        <span className="text-[14px] font-medium text-ink">{message}</span>
        <span className="mono max-w-full truncate text-[11.5px] text-ink-3">
          {path}
        </span>
      </motion.div>
    </div>
  );
}
