import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import {
  fetchNodeDetail,
  fetchSource,
  type NodeDetail,
  type ProcessMembership,
  type ProcessStep,
  type SourceResult,
} from "../repo-api";
import {
  fetchSymbolContext,
  type ContextRef,
  type SymbolContext,
} from "../search-api";
import { openEditorUrl } from "../desktop-api";
import { useAppStore, type DetailTarget } from "../store";
import EgoMiniGraph from "./EgoMiniGraph";

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
  const requestFocus = useAppStore((s) => s.requestFocus);
  const openCode = useAppStore((s) => s.openCode);
  const view = useAppStore((s) => s.view);
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

  const label = detail?.label || target.label || "Node";

  /* Process 合成流程节点：没有自身源码位置，源码区改用流程入口符号的位置。
     旧后端无 process 字段 → entry 为 null → 完全回退现状展示。 */
  const isProcess = label === "Process";
  const proc = (isProcess ? detail?.process : null) ?? null;
  const entry = proc?.entry ?? null;
  /** step 升序的流程步骤；空数组按无步骤处理（回退 refs 现状逻辑） */
  const steps =
    proc && proc.steps?.length
      ? [...proc.steps].sort((a, b) => a.step - b.step)
      : null;
  /** 该符号参与的流程（普通节点；空数组不渲染） */
  const memberships = detail?.processes?.length ? detail.processes : null;

  /* 源码片段：文件/行号优先用 /api/node 的精确值，回退点击来源自带的；
     Process 节点自身无位置时回退流程入口的位置。 */
  const ownFile = detail?.file || target.file;
  const ownLine = detail?.line || target.line;
  const file = ownFile || (entry?.file ?? "");
  const line = ownFile ? ownLine : (entry?.line ?? 0);
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

  /* 关系/流程跳转时打开 Code 视图（GitHub 式全文预览） */
  const repoName = repos.find((r) => r.id === repoId)?.name ?? repoId;

  /* 点击关系条目：Code 视图里同时把中栏跳到该节点（跟着图谱走），
     并把抽屉重锚到它；Graph 视图里只重锚，不抢占画布。 */
  const pickRelation = (r: ContextRef) => {
    if (view === "code" && r.file) {
      openCode({
        repo: repoName,
        path: r.file,
        line: r.line > 0 ? r.line : undefined,
      });
    }
    openDetail({
      id: r.id,
      name: r.name,
      label: r.label,
      file: r.file,
      line: r.line,
    });
  };

  /* 流程步骤行 → 选中该符号节点（与 refs 行为一致） */
  const pickStep = (s: ProcessStep) => {
    openDetail({
      id: s.id,
      name: s.name,
      label: s.label,
      file: s.file,
      line: s.line,
    });
  };

  /* 参与流程行 → 选中该 Process 节点（process_id 即节点 id） */
  const pickProcess = (m: ProcessMembership) => {
    openDetail({
      id: m.process_id,
      name: m.name,
      label: "Process",
      file: "",
      line: 0,
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
      className="glass-panel absolute bottom-3 right-3 top-14 z-40 flex flex-col overflow-hidden"
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
          {(ownFile || ownLine > 0) && (
            <div
              className="mono mt-1 truncate text-[11px] text-ink-3"
              title={ownFile}
            >
              {ownFile}
              {ownLine > 0 ? `:${ownLine}` : ""}
              {endLine > ownLine ? `–${endLine}` : ""}
            </div>
          )}
        </div>
        <button
          onClick={closeDetail}
          aria-label="关闭详情面板"
          className="themed-hover focus-ring -mr-1 flex h-6 w-6 flex-none items-center justify-center rounded-[7px] text-[15px] leading-none text-ink-3 transition-colors duration-150 ease-out hover:text-ink"
          data-testid="detail-close"
        >
          ×
        </button>
      </div>

      {/* ---- 内容（滚动区） ---- */}
      <div className="scroll-area min-h-0 flex-1 px-4 pb-3">
        {/* 流程元信息（Process 合成节点） */}
        {proc && (
          <div
            className="mb-3 flex items-center gap-2"
            data-testid="process-meta"
          >
            <span className="badge Process">
              {processTypeText(proc.process_type)}
            </span>
            <span className="tabular text-[11px] text-ink-3">
              {proc.step_count} 步
            </span>
          </div>
        )}

        {/* 顶部互补视图：Code 模式 = ego 图谱；Graph 模式 = 源码预览 */}
        {view === "code" ? (
          <>
            <div className="mb-1.5 flex items-baseline">
              <span className="text-[10.5px] font-semibold uppercase tracking-[0.08em] text-ink-3">
                Graph
              </span>
            </div>
            <EgoMiniGraph
              centerName={target.name || target.id}
              callers={ctx?.callers ?? []}
              callees={ctx?.callees ?? []}
              refs={ctx?.refs ?? []}
              loading={relationsPending}
              onPick={pickRelation}
            />
          </>
        ) : (
          <>
            <div className="mb-1.5 flex items-baseline">
              <span className="text-[10.5px] font-semibold uppercase tracking-[0.08em] text-ink-3">
                Source
              </span>
            </div>
            {entry && !ownFile && (
              <div
                className="mono mb-1.5 flex items-baseline gap-2 px-0.5 text-[11px]"
                data-testid="process-entry"
              >
                <span className="truncate text-ink-2" title={entry.file}>
                  {entry.file}
                  {entry.line > 0 ? `:${entry.line}` : ""}
                </span>
                <span className="flex-none text-[10px] text-ink-3">流程入口</span>
              </div>
            )}
            <SourcePreview
              source={source}
              hasFile={Boolean(file)}
              resolving={!detailDone}
              file={file}
              focusStart={line}
              focusEnd={endLine}
              missingNote={
                isProcess
                  ? "合成流程节点，无单一源码位置"
                  : "该节点没有关联的源码位置"
              }
            />
          </>
        )}

        {steps ? (
          <>
            {/* 流程步骤时间线（替代对 Process 无意义的 callers/callees/refs） */}
            <SectionTitle className="mt-4">流程步骤</SectionTitle>
            <ProcessStepList steps={steps} onPick={pickStep} />
          </>
        ) : (
          <>
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
          </>
        )}

        {/* 参与流程（普通符号节点；空数组不渲染） */}
        {memberships && (
          <>
            <SectionTitle className="mt-4">参与流程</SectionTitle>
            <div className="mb-2">
              {memberships.map((m, i) => (
                <button
                  key={`${m.process_id}-${i}`}
                  onClick={() => pickProcess(m)}
                  className="focus-ring mb-0.5 flex w-full items-center gap-2 rounded-[9px] px-2 py-1.5 text-left transition-colors duration-150 ease-out hover:bg-[rgba(46,124,246,0.07)]"
                  data-testid="process-membership-row"
                >
                  <span
                    className="h-[7px] w-[7px] flex-none rounded-full"
                    style={{ background: "rgba(52,199,89,0.55)" }}
                  />
                  <span className="mono truncate text-[12px] text-ink">
                    {m.name}
                  </span>
                  <span className="tabular ml-auto flex-none text-[10.5px] text-ink-3">
                    第 {m.step}/共 {m.step_count} 步
                  </span>
                </button>
              ))}
            </div>
          </>
        )}
      </div>

      {/* ---- 操作 ---- */}
      <div className="themed-divider grid grid-cols-2 gap-1.5 border-t px-4 py-3">
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
          onClick={() => requestFocus(target.id, target.name || target.id)}
          className="btn-primary focus-ring px-3 py-2 text-[12.5px] font-semibold"
          data-testid="detail-focus-graph"
        >
          在 Graph 定位
        </button>
        <ActionButton
          onClick={() => requestEgo(target.id, target.name || target.id)}
          testId="detail-ego"
        >
          Ego 视图
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
      style={{ background: "var(--subtle-fill-2)" }}
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
          className="focus-ring mb-0.5 flex w-full items-center gap-2 rounded-[9px] px-2 py-1.5 text-left transition-colors duration-150 ease-out hover:bg-[var(--accent-fill)]"
          data-testid="relation-row"
        >
          <span
            className="h-[7px] w-[7px] flex-none rounded-full"
            style={{
              background:
                r.depth <= 1
                  ? "var(--accent)"
                  : "color-mix(in srgb, var(--accent) 42%, transparent)",
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

/** process_type → 中文徽章文案（未知类型回退通用文案） */
function processTypeText(t: string): string {
  if (t === "cross_community") return "跨社区流程";
  if (t === "intra_community") return "社区内流程";
  return "流程";
}

/** Process 节点的步骤时间线：步号 + 符号名 + 文件短名:行，点击选中该节点。 */
function ProcessStepList({
  steps,
  onPick,
}: {
  steps: ProcessStep[];
  onPick(s: ProcessStep): void;
}) {
  return (
    <div className="mb-2" data-testid="process-steps">
      {steps.map((s, i) => (
        <button
          key={`${s.id}-${s.step}-${i}`}
          onClick={() => onPick(s)}
          className="focus-ring mb-0.5 flex w-full items-center gap-2 rounded-[9px] px-2 py-1.5 text-left transition-colors duration-150 ease-out hover:bg-[var(--accent-fill)]"
          data-testid="process-step-row"
        >
          <span
            className="tabular flex h-[18px] min-w-[18px] flex-none items-center justify-center rounded-full px-1 text-[10px] font-semibold text-ink-3"
            style={{
              background: "var(--subtle-fill)",
              boxShadow: "inset 0 0 0 0.5px var(--hairline)",
            }}
            aria-hidden
          >
            {s.step}
          </span>
          <span className="mono truncate text-[12px] text-ink">{s.name}</span>
          {s.file && (
            <span className="mono tabular ml-auto flex-none text-[10.5px] text-ink-3">
              {s.file.split("/").pop()}
              {s.line > 0 ? `:${s.line}` : ""}
            </span>
          )}
        </button>
      ))}
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
      className="themed-hover focus-ring rounded-[10px] px-3 py-2 text-[12.5px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:text-ink disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent disabled:hover:text-ink-2"
      style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)" }}
      data-testid={testId}
    >
      {children}
    </button>
  );
}

/** 打开源码位置；Tauri 内走系统 opener，避免 webview 拦截 vscode:// scheme。 */
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
        style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline)" }}
        title="源码路径不可用（需 aka serve 在线）"
        data-testid={testId}
      >
        {children}
      </span>
    );
  }
  return (
    <button
      type="button"
      onClick={() => void openEditorUrl(href)}
      className="themed-hover focus-ring rounded-[10px] px-3 py-2 text-center text-[12.5px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:text-ink"
      style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)" }}
      data-testid={testId}
    >
      {children}
    </button>
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
  missingNote,
}: {
  source: SourceResult | null;
  hasFile: boolean;
  /** /api/node 仍在加载（可能补全文件路径） */
  resolving: boolean;
  file: string;
  focusStart: number;
  focusEnd: number;
  /** 无源码位置时的提示文案（Process 合成节点用专属文案） */
  missingNote: string;
}) {
  if (!hasFile) {
    return resolving ? <SourceSkeleton /> : <SourceUnavailable note={missingNote} />;
  }
  if (source === null) return <SourceSkeleton />;
  if (source.state === "unsupported") {
    return <SourceUnavailable note="源码不可用——后端暂不支持 /api/source" />;
  }
  if (source.state === "binary") {
    return <SourceUnavailable note="非文本文件，无法预览" />;
  }
  if (source.state === "missing") {
    return <SourceUnavailable note="源码文件不存在或索引已过期" />;
  }
  if (source.state === "error") {
    return <SourceUnavailable note={`源码读取失败——${source.message}`} />;
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
  return (
    <div
      className="overflow-hidden rounded-[10px]"
      style={{
        background: "var(--code-bg)",
        boxShadow: "inset 0 0 0 0.5px var(--hairline)",
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
                  ? { background: "var(--code-line-focus)" }
                  : undefined
              }
            >
              <span
                className="sticky left-0 z-[1] flex-none select-none text-right text-ink-3"
                style={{
                  width: gutterWidth,
                  paddingLeft: 10,
                  paddingRight: 12,
                  background: focused
                    ? "var(--code-gutter-focus)"
                    : "var(--code-gutter)",
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
        <div className="themed-divider border-t px-2.5 py-1 text-[10px] text-ink-3">
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
        background: "var(--code-bg)",
        boxShadow: "inset 0 0 0 0.5px var(--hairline)",
      }}
      data-testid="source-skeleton"
    >
      {widths.map((w, i) => (
        <div
          key={i}
          className="h-[10px] animate-pulse rounded-[4px]"
          style={{ width: w, background: "var(--subtle-fill)" }}
        />
      ))}
    </div>
  );
}

function SourceUnavailable({ note }: { note: string }) {
  return (
    <div
      className="rounded-[10px] px-3 py-2.5 text-[11.5px] text-ink-3"
      style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline)" }}
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
    string: { color: "var(--syntax-string)" },
    number: { color: "var(--syntax-number)" },
    keyword: { color: "var(--syntax-keyword)" },
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
