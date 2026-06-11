import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import {
  fetchNodeDetail,
  fetchSource,
  type NodeDetail,
  type SourceResult,
} from "../repo-api";
import {
  fetchSymbolContext,
  type ContextRef,
  type SymbolContext,
} from "../search-api";
import { useAppStore, type DetailTarget } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;
/** 源码预览：目标行前后各取多少行上下文 */
const CONTEXT_LINES = 20;
/** 每组关系最多列出多少条可点条目 */
const RELATION_ROWS = 6;

/* ---- 面板宽度可调（左缘拖拽手柄） ---- */
const WIDTH_KEY = "aka.detailWidth";
const DEFAULT_WIDTH = 400;
const MIN_WIDTH = 320;

function maxPanelWidth(): number {
  return Math.min(window.innerWidth * 0.7, 960);
}

function clampWidth(w: number): number {
  return Math.min(Math.max(w, MIN_WIDTH), maxPanelWidth());
}

function readPersistedWidth(): number {
  try {
    const saved = Number(localStorage.getItem(WIDTH_KEY));
    if (Number.isFinite(saved) && saved > 0) return clampWidth(saved);
  } catch {
    /* localStorage 不可用——用默认宽度 */
  }
  return DEFAULT_WIDTH;
}

function persistWidth(w: number) {
  try {
    localStorage.setItem(WIDTH_KEY, String(Math.round(w)));
  } catch {
    /* 静默忽略 */
  }
}

/**
 * 右侧详情侧边栏 —— Search / Graph / Symbol 三视图共用。
 * 数据三路并发拉取，全部按离线/旧后端优雅降级：
 *   GET  /api/node           → degree / 精确行号
 *   POST /api/symbol/context → callers / callees / refs 条目
 *   GET  /api/source         → 源码片段（abs_path 供编辑器跳转）
 */
export default function DetailPanel() {
  const target = useAppStore((s) => s.detailTarget);
  const repoId = useAppStore((s) => s.selectedRepoId);
  return (
    <AnimatePresence>
      {target && <PanelBody target={target} repoId={repoId} />}
    </AnimatePresence>
  );
}

function PanelBody({
  target,
  repoId,
}: {
  target: DetailTarget;
  repoId: string;
}) {
  const closeDetail = useAppStore((s) => s.closeDetail);
  const openDetail = useAppStore((s) => s.openDetail);
  const requestEgo = useAppStore((s) => s.requestEgo);
  const setQuery = useAppStore((s) => s.setQuery);
  const setView = useAppStore((s) => s.setView);
  const openCode = useAppStore((s) => s.openCode);
  const repos = useAppStore((s) => s.repos);
  const panelRef = useRef<HTMLElement>(null);

  /* 面板宽度：localStorage 持久化，拖拽手柄实时调整 */
  const [width, setWidth] = useState(readPersistedWidth);
  const [resizing, setResizing] = useState(false);
  const dragRef = useRef<{ startX: number; startWidth: number } | null>(null);

  const [detail, setDetail] = useState<NodeDetail | null>(null);
  const [detailDone, setDetailDone] = useState(false);
  const [ctx, setCtx] = useState<SymbolContext | null>(null);
  const [ctxDone, setCtxDone] = useState(false);
  /** null = 加载中 */
  const [source, setSource] = useState<SourceResult | null>(null);
  const [copied, setCopied] = useState(false);

  /* 拖拽调宽期间全局 col-resize 光标（pointer capture 下指针可能离开手柄） */
  useEffect(() => {
    if (!resizing) return;
    const prev = document.body.style.cursor;
    document.body.style.cursor = "col-resize";
    return () => {
      document.body.style.cursor = prev;
    };
  }, [resizing]);

  /* Esc 关闭 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeDetail();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [closeDetail]);

  /* 点击面板外关闭（点结果行会在随后的 click 里重新打开）。
     带 data-detail-manage 的区域（GraphView 画布）自己管理选中/关闭，
     这里跳过——否则拖拽平移会误关面板。 */
  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (!(e.target instanceof Element)) return;
      if (panelRef.current?.contains(e.target)) return;
      if (e.target.closest("[data-detail-manage]")) return;
      closeDetail();
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [closeDetail]);

  /* 节点详情 + 符号上下文（并发，互不阻塞） */
  useEffect(() => {
    let stale = false;
    setDetail(null);
    setDetailDone(false);
    setCtx(null);
    setCtxDone(false);
    setCopied(false);
    void fetchNodeDetail(repoId, target.id).then((res) => {
      if (stale) return;
      if (res.state === "ok") setDetail(res.detail);
      setDetailDone(true);
    });
    if (target.name) {
      void fetchSymbolContext(target.name, repoId).then((out) => {
        if (stale) return;
        setCtx(out === "offline" || out === null ? null : out);
        setCtxDone(true);
      });
    } else {
      setCtxDone(true);
    }
    return () => {
      stale = true;
    };
  }, [repoId, target.id, target.name]);

  /* 源码片段：文件/行号优先用 /api/node 的精确值，回退点击来源自带的 */
  const file = detail?.file || target.file;
  const line = detail?.line || target.line;
  const endLine = Math.max(detail?.end_line ?? 0, line);

  useEffect(() => {
    if (!file) {
      setSource(null);
      return;
    }
    let stale = false;
    setSource(null);
    const focus = Math.max(1, line || 1);
    void fetchSource(
      repoId,
      file,
      Math.max(1, focus - CONTEXT_LINES),
      focus + CONTEXT_LINES,
    ).then((res) => {
      if (!stale) setSource(res);
    });
    return () => {
      stale = true;
    };
  }, [repoId, file, line]);

  const label = detail?.label || target.label || "Node";
  const absPath = source?.state === "ok" ? source.source.abs_path : null;
  const copyText = absPath || file;

  const copyPath = () => {
    if (!copyText) return;
    void navigator.clipboard
      .writeText(copyText)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1600);
      })
      .catch(() => {
        /* 剪贴板不可用（权限/非安全上下文）——静默忽略 */
      });
  };

  const open360 = () => {
    if (!target.name) return;
    setQuery(target.name);
    setView("doc");
    closeDetail();
  };

  /* 「查看完整文件」→ Code 视图（GitHub 式全文预览，CodeView 另行实现） */
  const repoName = repos.find((r) => r.id === repoId)?.name ?? repoId;
  const openFullFile = () => {
    if (!file) return;
    openCode({
      repo: repoName,
      path: file,
      line: line > 0 ? line : undefined,
      endLine: line > 0 ? endLine : undefined,
    });
    closeDetail();
  };

  const pickRelation = (r: ContextRef) => {
    openDetail({
      id: r.id,
      name: r.name,
      label: r.label,
      file: r.file,
      line: r.line,
    });
  };

  /* 关系计数：context 列表优先，缺失时回退 /api/node 的 degree */
  const counts = {
    callers: ctx ? ctx.callers.length : (detail?.degree.callers ?? null),
    callees: ctx ? ctx.callees.length : (detail?.degree.callees ?? null),
    refs: ctx ? ctx.refs.length : (detail?.degree.refs ?? null),
  };
  const relationsPending = !ctxDone || (!ctx && !detailDone);

  return (
    <motion.aside
      ref={panelRef}
      initial={{ x: 56, opacity: 0 }}
      animate={{ x: 0, opacity: 1 }}
      exit={{ x: 56, opacity: 0 }}
      transition={spring}
      className="glass-panel absolute bottom-3 right-3 top-3 z-40 flex flex-col overflow-hidden"
      style={{
        width,
        minWidth: MIN_WIDTH,
        maxWidth: "min(70vw, 960px, calc(100% - 1.5rem))",
      }}
      data-graph-ui
      data-testid="detail-panel"
      aria-label={`${target.name} 详情`}
    >
      {/* ---- 左缘拖拽手柄：调整面板宽度 ---- */}
      <div
        role="separator"
        aria-orientation="vertical"
        aria-label="调整面板宽度"
        className="group absolute inset-y-0 left-0 z-50 w-2.5 cursor-col-resize touch-none"
        data-testid="detail-resize-handle"
        onPointerDown={(e) => {
          e.preventDefault();
          e.currentTarget.setPointerCapture(e.pointerId);
          dragRef.current = { startX: e.clientX, startWidth: width };
          setResizing(true);
        }}
        onPointerMove={(e) => {
          const d = dragRef.current;
          if (!d) return;
          setWidth(clampWidth(d.startWidth + (d.startX - e.clientX)));
        }}
        onPointerUp={(e) => {
          const d = dragRef.current;
          if (!d) return;
          dragRef.current = null;
          setResizing(false);
          const final = clampWidth(d.startWidth + (d.startX - e.clientX));
          setWidth(final);
          persistWidth(final);
        }}
        onPointerCancel={() => {
          dragRef.current = null;
          setResizing(false);
        }}
      >
        <div
          className="absolute bottom-2 left-[3px] top-2 w-[3px] rounded-full bg-transparent transition-colors duration-150 ease-out group-hover:bg-[rgba(46,124,246,0.4)]"
          style={resizing ? { background: "rgba(46,124,246,0.55)" } : undefined}
        />
      </div>

      {/* ---- 头部 ---- */}
      <div className="flex items-start gap-2 px-4 pb-2.5 pt-3.5">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="mono truncate text-[14px] font-semibold text-ink">
              {target.name || target.id}
            </span>
            <span className={`badge ${label}`}>{label}</span>
          </div>
          {(file || line > 0) && (
            <div
              className="mono mt-1 truncate text-[11px] text-ink-3"
              title={file}
            >
              {file}
              {line > 0 ? `:${line}` : ""}
              {endLine > line ? `–${endLine}` : ""}
            </div>
          )}
        </div>
        <button
          onClick={closeDetail}
          aria-label="关闭详情面板"
          className="focus-ring -mr-1 flex h-6 w-6 flex-none items-center justify-center rounded-[7px] text-[15px] leading-none text-ink-3 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)] hover:text-ink"
          data-testid="detail-close"
        >
          ×
        </button>
      </div>

      {/* ---- 内容（滚动区） ---- */}
      <div className="scroll-area min-h-0 flex-1 px-4 pb-3">
        {/* 源码预览 */}
        <div className="mb-1.5 flex items-baseline justify-between">
          <span className="text-[10.5px] font-semibold uppercase tracking-[0.08em] text-ink-3">
            Source
          </span>
          {file && (
            <button
              onClick={openFullFile}
              className="focus-ring -my-0.5 rounded-[6px] px-1.5 py-0.5 text-[10.5px] font-medium text-ink-3 transition-colors duration-150 ease-out hover:bg-[rgba(46,124,246,0.07)] hover:text-[#2E7CF6]"
              title={`在 Code 视图打开 ${file}`}
              data-testid="open-full-file"
            >
              查看完整文件 ↗
            </button>
          )}
        </div>
        <SourcePreview
          source={source}
          hasFile={Boolean(file)}
          resolving={!detailDone}
          file={file}
          focusStart={line}
          focusEnd={endLine}
        />

        {/* 关系 */}
        <SectionTitle className="mt-4">Relations</SectionTitle>
        <div className="mb-2 grid grid-cols-3 gap-1.5">
          <CountStat label="callers" value={counts.callers} pending={relationsPending} />
          <CountStat label="callees" value={counts.callees} pending={relationsPending} />
          <CountStat label="refs" value={counts.refs} pending={relationsPending} />
        </div>
        {ctx ? (
          <>
            <RelationGroup
              title="Callers"
              rows={ctx.callers}
              onPick={pickRelation}
            />
            <RelationGroup
              title="Callees"
              rows={ctx.callees}
              onPick={pickRelation}
            />
            <RelationGroup title="Refs" rows={ctx.refs} onPick={pickRelation} />
          </>
        ) : (
          ctxDone &&
          detailDone &&
          !detail && (
            <div
              className="rounded-[10px] px-3 py-2 text-[11.5px]"
              style={{ background: "rgba(246,166,35,0.1)", color: "#8a5a10" }}
              data-testid="detail-degraded"
            >
              关系数据不可用——aka serve 离线或后端版本过旧
            </div>
          )
        )}
      </div>

      {/* ---- 操作 ---- */}
      <div className="grid grid-cols-2 gap-1.5 border-t border-[rgba(15,23,42,0.06)] px-4 py-3">
        <ActionLink
          href={
            absPath
              ? `vscode://file/${encodeURI(absPath)}${line > 0 ? `:${line}` : ""}`
              : null
          }
          testId="open-in-editor"
        >
          在编辑器打开
        </ActionLink>
        <ActionButton
          onClick={copyPath}
          disabled={!copyText}
          testId="copy-path"
        >
          {copied ? "已复制 ✓" : "复制路径"}
        </ActionButton>
        <button
          onClick={() => requestEgo(target.id, target.name || target.id)}
          className="btn-primary focus-ring px-3 py-2 text-[12.5px] font-semibold"
          data-testid="detail-ego"
        >
          Ego 视图
        </button>
        <ActionButton
          onClick={open360}
          disabled={!target.name}
          testId="open-360"
        >
          打开 360° 视图
        </ActionButton>
      </div>
    </motion.aside>
  );
}

/* ============================== 子组件 ============================== */

function SectionTitle({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={`mb-1.5 text-[10.5px] font-semibold uppercase tracking-[0.08em] text-ink-3 ${className}`}
    >
      {children}
    </div>
  );
}

function CountStat({
  label,
  value,
  pending,
}: {
  label: string;
  value: number | null;
  pending: boolean;
}) {
  return (
    <div
      className="tabular flex flex-col items-center rounded-[10px] px-2 py-1.5"
      style={{ background: "rgba(15,23,42,0.04)" }}
    >
      <span className="text-[14px] font-semibold text-ink">
        {value !== null ? formatCount(value) : pending ? "…" : "—"}
      </span>
      <span className="text-[10px] text-ink-3">{label}</span>
    </div>
  );
}

function RelationGroup({
  title,
  rows,
  onPick,
}: {
  title: string;
  rows: ContextRef[];
  onPick(r: ContextRef): void;
}) {
  if (rows.length === 0) return null;
  const shown = rows.slice(0, RELATION_ROWS);
  return (
    <div className="mb-2">
      <div className="mb-0.5 flex items-baseline justify-between px-1">
        <span className="text-[11px] font-semibold text-ink-2">{title}</span>
        <span className="tabular text-[10.5px] text-ink-3">{rows.length}</span>
      </div>
      {shown.map((r, i) => (
        <button
          key={`${r.id}-${i}`}
          onClick={() => onPick(r)}
          className="focus-ring mb-0.5 flex w-full items-center gap-2 rounded-[9px] px-2 py-1.5 text-left transition-colors duration-150 ease-out hover:bg-[rgba(46,124,246,0.07)]"
          data-testid="relation-row"
        >
          <span
            className="h-[7px] w-[7px] flex-none rounded-full"
            style={{
              background: r.depth <= 1 ? "#2E7CF6" : "rgba(46,124,246,0.35)",
            }}
          />
          <span className="mono truncate text-[12px] text-ink">{r.name}</span>
          <span className="mono tabular ml-auto flex-none text-[10.5px] text-ink-3">
            {r.file.split("/").pop()}:{r.line}
          </span>
        </button>
      ))}
      {rows.length > shown.length && (
        <div className="px-2 py-0.5 text-[10.5px] text-ink-3">
          等 {rows.length} 项 · 完整列表见 360° 视图
        </div>
      )}
    </div>
  );
}

function ActionButton({
  onClick,
  disabled,
  testId,
  children,
}: {
  onClick(): void;
  disabled?: boolean;
  testId: string;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className="focus-ring rounded-[10px] px-3 py-2 text-[12.5px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)] hover:text-ink disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent disabled:hover:text-ink-2"
      style={{ boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.1)" }}
      data-testid={testId}
    >
      {children}
    </button>
  );
}

/** 链接型操作（vscode:// 在浏览器与 Tauri webview 中均可经 anchor 触发）。 */
function ActionLink({
  href,
  testId,
  children,
}: {
  href: string | null;
  testId: string;
  children: React.ReactNode;
}) {
  if (!href) {
    return (
      <span
        className="cursor-not-allowed rounded-[10px] px-3 py-2 text-center text-[12.5px] font-medium text-ink-3 opacity-60"
        style={{ boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.08)" }}
        title="源码路径不可用（需 aka serve 在线）"
        data-testid={testId}
      >
        {children}
      </span>
    );
  }
  return (
    <a
      href={href}
      className="focus-ring rounded-[10px] px-3 py-2 text-center text-[12.5px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)] hover:text-ink"
      style={{ boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.1)" }}
      data-testid={testId}
    >
      {children}
    </a>
  );
}

/* ============================== 源码预览 ============================== */

function SourcePreview({
  source,
  hasFile,
  resolving,
  file,
  focusStart,
  focusEnd,
}: {
  source: SourceResult | null;
  hasFile: boolean;
  /** /api/node 仍在加载（可能补全文件路径） */
  resolving: boolean;
  file: string;
  focusStart: number;
  focusEnd: number;
}) {
  if (!hasFile) {
    return resolving ? (
      <SourceSkeleton />
    ) : (
      <SourceUnavailable note="该节点没有关联的源码位置" />
    );
  }
  if (source === null) return <SourceSkeleton />;
  if (source.state === "unsupported") {
    return <SourceUnavailable note="源码不可用——后端暂不支持 /api/source" />;
  }
  if (source.state === "binary") {
    return <SourceUnavailable note="非文本文件，无法预览" />;
  }
  if (source.state === "offline") {
    return <SourceUnavailable note="源码不可用——aka serve 离线" />;
  }

  const { start, lines, total_lines, truncated } = source.source;
  const hash = hashComments(file);
  /* 行号栏：按最大行号位数定宽（含 10px 左 padding + 12px 右间距），
     横向滚动时 sticky 钉在左缘，不透明背景盖住滚过的代码（GitHub 做法）。 */
  const digits = Math.max(3, String(start + lines.length - 1).length);
  const gutterWidth = `calc(${digits}ch + 22px)`;
  /* sticky 行号背景需不透明（面板是毛玻璃）：取代码区底色在画布上的实色近似 */
  const gutterBg = "rgb(246,247,249)";
  const gutterBgFocused = "rgb(229,237,250)";

  return (
    <div
      className="overflow-hidden rounded-[10px]"
      style={{
        background: "rgba(15,23,42,0.025)",
        boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.07)",
      }}
      data-testid="source-preview"
    >
      <div className="overflow-x-auto py-1.5">
        {lines.map((text, i) => {
          const ln = start + i;
          const focused = ln >= focusStart && ln <= focusEnd && focusStart > 0;
          return (
            <div
              key={ln}
              className="mono flex w-max min-w-full text-[11px] leading-[1.65]"
              style={
                focused
                  ? { background: "rgba(46,124,246,0.08)" }
                  : undefined
              }
            >
              <span
                className="sticky left-0 z-[1] flex-none select-none text-right text-ink-3"
                style={{
                  width: gutterWidth,
                  paddingLeft: 10,
                  paddingRight: 12,
                  background: focused ? gutterBgFocused : gutterBg,
                  boxShadow: focused
                    ? "inset 2px 0 0 rgba(46,124,246,0.55)"
                    : undefined,
                }}
                aria-hidden
              >
                {ln}
              </span>
              <span className="whitespace-pre pr-3 text-ink-2">
                {renderTokens(text, hash)}
              </span>
            </div>
          );
        })}
      </div>
      {(truncated || total_lines > 0) && (
        <div className="border-t border-[rgba(15,23,42,0.05)] px-2.5 py-1 text-[10px] text-ink-3">
          {truncated ? "片段已截断 · " : ""}全文件 {total_lines} 行
        </div>
      )}
    </div>
  );
}

function SourceSkeleton() {
  const widths = ["72%", "94%", "58%", "85%", "66%", "90%", "45%"];
  return (
    <div
      className="space-y-2 rounded-[10px] px-3 py-3"
      style={{
        background: "rgba(15,23,42,0.025)",
        boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.07)",
      }}
      data-testid="source-skeleton"
    >
      {widths.map((w, i) => (
        <div
          key={i}
          className="h-[10px] animate-pulse rounded-[4px]"
          style={{ width: w, background: "rgba(15,23,42,0.06)" }}
        />
      ))}
    </div>
  );
}

function SourceUnavailable({ note }: { note: string }) {
  return (
    <div
      className="rounded-[10px] px-3 py-2.5 text-[11.5px] text-ink-3"
      style={{ boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.07)" }}
      data-testid="source-unavailable"
    >
      {note}
    </div>
  );
}

/* ---- 轻量级语法高亮：纯正则 token，不引入任何高亮库 ---- */

const KEYWORDS = new Set(
  (
    "fn let mut pub use mod impl struct enum trait match if else for while loop return " +
    "async await const static type where dyn ref move crate super function class interface " +
    "extends implements import export from new this var void typeof instanceof in of try " +
    "catch finally throw switch case break continue default do yield def lambda pass raise " +
    "with as global elif not and or is None True False null undefined true false package " +
    "func go chan defer select range int string bool float double char long public private " +
    "protected abstract final override readonly namespace virtual delete union self Self"
  ).split(" "),
);

type TokenClass = "comment" | "string" | "number" | "keyword" | "plain";

const TOKEN_COLORS: Record<Exclude<TokenClass, "plain">, React.CSSProperties> =
  {
    comment: { color: "var(--ink-3)", fontStyle: "italic" },
    string: { color: "#9d5b25" },
    number: { color: "#7c4fc9" },
    keyword: { color: "#2563c9" },
  };

const TOKEN_RE =
  /\/\/.*|\/\*.*?(?:\*\/|$)|"(?:\\.|[^"\\])*"?|'(?:\\.|[^'\\])*'?|`(?:\\.|[^`\\])*`?|\b\d[\d_]*(?:\.\d+)?\b|\b[A-Za-z_$][\w$]*\b/g;

/** 以 # 开头注释的语言（按扩展名粗判，误判代价低） */
function hashComments(file: string): boolean {
  return /\.(py|rb|sh|bash|zsh|pl|yml|yaml|toml|cfg|ini|mk)$|Makefile$|Dockerfile$/i.test(
    file,
  );
}

function classify(tok: string, hash: boolean): TokenClass {
  const c0 = tok[0];
  if (c0 === "/" && (tok[1] === "/" || tok[1] === "*")) return "comment";
  if (c0 === '"' || c0 === "'" || c0 === "`") return "string";
  if (c0 >= "0" && c0 <= "9") return "number";
  if (KEYWORDS.has(tok)) return "keyword";
  void hash;
  return "plain";
}

function renderTokens(text: string, hash: boolean): React.ReactNode {
  /* # 注释：整行剩余部分视为注释（仅 hash 语言） */
  let head = text;
  let hashTail: string | null = null;
  if (hash) {
    const idx = text.indexOf("#");
    if (idx >= 0) {
      head = text.slice(0, idx);
      hashTail = text.slice(idx);
    }
  }

  const out: React.ReactNode[] = [];
  let last = 0;
  let key = 0;
  TOKEN_RE.lastIndex = 0;
  for (let m = TOKEN_RE.exec(head); m; m = TOKEN_RE.exec(head)) {
    if (m.index > last) out.push(head.slice(last, m.index));
    const cls = classify(m[0], hash);
    if (cls === "plain") {
      out.push(m[0]);
    } else {
      out.push(
        <span key={key++} style={TOKEN_COLORS[cls]}>
          {m[0]}
        </span>,
      );
    }
    last = m.index + m[0].length;
  }
  if (last < head.length) out.push(head.slice(last));
  if (hashTail !== null) {
    out.push(
      <span key={key++} style={TOKEN_COLORS.comment}>
        {hashTail}
      </span>,
    );
  }
  return out;
}

function formatCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}
