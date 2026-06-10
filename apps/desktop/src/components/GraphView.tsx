import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { Camera } from "../graph/camera";
import { generateDemoGraph } from "../graph/demo";
import { loadEgoGraph, loadRealGraph } from "../graph/source";
import type { GraphData } from "../graph/format";
import { SpatialGrid } from "../graph/grid";
import { LabelOverlay } from "../graph/labels";
import {
  ACCENT,
  BEACON,
  computeLod,
  GraphRenderer,
  hexToRgb,
  type OverlayItem,
} from "../graph/renderer";
import { fetchNodeDetail, type NodeDetail } from "../repo-api";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

const ACCENT_RGB = hexToRgb(ACCENT);
const BEACON_RGB = hexToRgb(BEACON);
const LOD_NAMES = ["far", "mid", "near"] as const;
const BEACON_COUNT = 12;
const EGO_DEPTH = 2;

interface Stats {
  fps: number;
  lod: 0 | 1 | 2;
  nodes: number;
  edges: number;
}

interface HoverInfo {
  x: number;
  y: number;
  name: string;
  label: string;
  file: string;
}

/** 选中节点 — 来自图数据本身的基础信息（详情另拉 /api/node）。 */
interface PanelInfo {
  index: number;
  id: string;
  name: string;
  label: string;
  file: string;
}

interface EgoState {
  repoId: string;
  id: string;
  name: string;
}

type DetailState = "idle" | "loading" | "ok" | "degraded";

interface Rig {
  renderer: GraphRenderer;
  camera: Camera;
  labels: LabelOverlay;
  data: GraphData | null;
  grid: SpatialGrid | null;
  beacons: number[];
  /** ego 模式中心节点（i=0），-1 = 非 ego */
  centerIndex: number;
  hoverIndex: number;
  selectedIndex: number;
  mouseX: number;
  mouseY: number;
  mouseDirty: boolean;
  dragging: boolean;
  moved: boolean;
  applyData(data: GraphData): void;
}

export default function GraphView() {
  const containerRef = useRef<HTMLDivElement>(null);
  const glRef = useRef<HTMLCanvasElement>(null);
  const labelRef = useRef<HTMLCanvasElement>(null);
  const rigRef = useRef<Rig | null>(null);
  const onSelectRef = useRef<(i: number) => void>(() => {});

  const repoId = useAppStore((s) => s.selectedRepoId);
  const setView = useAppStore((s) => s.setView);
  const setQuery = useAppStore((s) => s.setQuery);

  const [stats, setStats] = useState<Stats>({ fps: 0, lod: 0, nodes: 0, edges: 0 });
  const [hover, setHover] = useState<HoverInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [live, setLive] = useState(false);
  const [panel, setPanel] = useState<PanelInfo | null>(null);
  const [detail, setDetail] = useState<NodeDetail | null>(null);
  const [detailState, setDetailState] = useState<DetailState>("idle");
  const [degradeNote, setDegradeNote] = useState("");
  const [ego, setEgo] = useState<EgoState | null>(null);
  const [egoError, setEgoError] = useState<string | null>(null);

  /* ---- rig：渲染器 / 相机 / 交互，仅挂载一次 ---- */
  useEffect(() => {
    const container = containerRef.current!;
    const glCanvas = glRef.current!;
    const labelCanvas = labelRef.current!;

    const camera = new Camera();
    const renderer = new GraphRenderer(glCanvas);
    const labels = new LabelOverlay(labelCanvas);
    const rig: Rig = {
      renderer,
      camera,
      labels,
      data: null,
      grid: null,
      beacons: [],
      centerIndex: -1,
      hoverIndex: -1,
      selectedIndex: -1,
      mouseX: 0,
      mouseY: 0,
      mouseDirty: false,
      dragging: false,
      moved: false,
      applyData(data: GraphData) {
        const grid = new SpatialGrid(data.positions, data.count, data.bounds);

        /* beacons: highest-degree hubs across distinct clusters */
        const order = Array.from(
          { length: data.clusterMeta.length },
          (_, c) => c,
        ).sort(
          (a, b) => data.clusterMeta[b].weight - data.clusterMeta[a].weight,
        );
        const beacons: number[] = [];
        for (const c of order.slice(0, BEACON_COUNT)) {
          let best = -1;
          let bestDeg = -1;
          grid.forEachInRect(
            data.clusterMeta[c].x - 40,
            data.clusterMeta[c].y - 40,
            data.clusterMeta[c].x + 40,
            data.clusterMeta[c].y + 40,
            256,
            (i) => {
              if (data.degrees[i] > bestDeg) {
                bestDeg = data.degrees[i];
                best = i;
              }
            },
          );
          if (best >= 0) beacons.push(best);
        }

        rig.data = data;
        rig.grid = grid;
        rig.beacons = beacons;
        rig.hoverIndex = -1;
        rig.selectedIndex = -1;
        setHover(null);
        renderer.setData(data);
        camera.fitBounds(data.bounds, true);
        setStats((s) => ({ ...s, nodes: data.count, edges: data.edgeCount }));
      },
    };
    rigRef.current = rig;
    let disposed = false;

    const applySize = () => {
      const rect = container.getBoundingClientRect();
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      camera.setViewport(rect.width, rect.height);
      renderer.resize(rect.width, rect.height, dpr);
      labels.resize(rect.width, rect.height, dpr);
    };
    applySize();
    const ro = new ResizeObserver(applySize);
    ro.observe(container);

    /* ---- interaction ---- */
    let lastX = 0;
    let lastY = 0;
    const velocitySamples: { t: number; x: number; y: number }[] = [];

    const toLocal = (e: PointerEvent | WheelEvent) => {
      const rect = container.getBoundingClientRect();
      return { x: e.clientX - rect.left, y: e.clientY - rect.top };
    };

    /* Native listeners on `container` fire before React's root-delegated
       handlers, so a React-level stopPropagation inside the floating
       controls can never reach us — skip events aimed at UI chrome
       ourselves, otherwise setPointerCapture steals the button's click. */
    const isUiTarget = (e: Event) =>
      e.target instanceof Element &&
      e.target.closest("button, [data-graph-ui]") !== null;

    const onPointerDown = (e: PointerEvent) => {
      if (e.button !== 0 || isUiTarget(e)) return;
      try {
        container.setPointerCapture(e.pointerId);
      } catch {
        /* pointer already gone (or synthetic) — drag still works via bubbling */
      }
      const p = toLocal(e);
      rig.dragging = true;
      rig.moved = false;
      lastX = p.x;
      lastY = p.y;
      velocitySamples.length = 0;
      velocitySamples.push({ t: performance.now(), x: p.x, y: p.y });
      camera.stop();
    };

    const onPointerMove = (e: PointerEvent) => {
      if (!rig.dragging && isUiTarget(e)) {
        /* hovering UI chrome — drop any graph hover state */
        rig.mouseDirty = false;
        if (rig.hoverIndex !== -1) {
          rig.hoverIndex = -1;
          setHover(null);
        }
        return;
      }
      const p = toLocal(e);
      rig.mouseX = p.x;
      rig.mouseY = p.y;
      rig.mouseDirty = true;
      if (!rig.dragging) return;
      const dx = p.x - lastX;
      const dy = p.y - lastY;
      if (
        Math.abs(p.x - velocitySamples[0].x) > 3 ||
        Math.abs(p.y - velocitySamples[0].y) > 3
      ) {
        rig.moved = true;
      }
      camera.panBy(dx, dy);
      lastX = p.x;
      lastY = p.y;
      const now = performance.now();
      velocitySamples.push({ t: now, x: p.x, y: p.y });
      while (velocitySamples.length > 2 && now - velocitySamples[0].t > 90) {
        velocitySamples.shift();
      }
    };

    const onPointerUp = (e: PointerEvent) => {
      if (!rig.dragging) return;
      rig.dragging = false;
      try {
        container.releasePointerCapture(e.pointerId);
      } catch {
        /* capture may never have been taken */
      }
      const now = performance.now();
      const first = velocitySamples[0];
      const last = velocitySamples[velocitySamples.length - 1];
      const dt = (now - first.t) / 1000;
      if (rig.moved && dt > 0.004) {
        const vx = (last.x - first.x) / dt;
        const vy = (last.y - first.y) / dt;
        if (Math.hypot(vx, vy) > 120) camera.fling(vx, vy);
      }
      if (!rig.moved) {
        /* click — select + open detail panel */
        const p = toLocal(e);
        const i = pickAt(rig, p.x, p.y);
        rig.selectedIndex = i;
        onSelectRef.current(i);
      }
    };

    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const p = toLocal(e);
      const speed = e.ctrlKey ? 0.0105 : 0.0024; /* ctrl = trackpad pinch */
      const factor = Math.exp(-e.deltaY * speed);
      camera.zoomAt(p.x, p.y, factor);
    };

    const onLeave = () => {
      rig.hoverIndex = -1;
      setHover(null);
    };

    container.addEventListener("pointerdown", onPointerDown);
    container.addEventListener("pointermove", onPointerMove);
    container.addEventListener("pointerup", onPointerUp);
    container.addEventListener("pointerleave", onLeave);
    container.addEventListener("wheel", onWheel, { passive: false });

    /* ---- frame loop ---- */
    let raf = 0;
    let lastT = performance.now();
    let frames = 0;
    let fpsWindowStart = lastT;
    const overlay: OverlayItem[] = [];

    const frame = (t: number) => {
      if (disposed) return;
      const dt = Math.min((t - lastT) / 1000, 1 / 20);
      lastT = t;
      camera.update(dt);
      const lod = computeLod(camera.zoomLevel);

      /* hover picking — at most once per frame */
      if (rig.mouseDirty && !rig.dragging && rig.grid && rig.data) {
        rig.mouseDirty = false;
        const i = pickAt(rig, rig.mouseX, rig.mouseY);
        if (i !== rig.hoverIndex) {
          rig.hoverIndex = i;
          if (i >= 0) {
            const d = rig.data;
            setHover({
              x: rig.mouseX,
              y: rig.mouseY,
              name: d.name(i),
              label: d.classNames[d.classes[i]] ?? "Node",
              file: d.file(i),
            });
          } else {
            setHover(null);
          }
        } else if (i >= 0) {
          setHover((h) => (h ? { ...h, x: rig.mouseX, y: rig.mouseY } : h));
        }
      }

      overlay.length = 0;
      for (const b of rig.beacons) {
        overlay.push({ i: b, color: BEACON_RGB, intensity: 0.5 });
      }
      if (rig.centerIndex >= 0) {
        overlay.push({ i: rig.centerIndex, color: BEACON_RGB, intensity: 1 });
      }
      if (rig.selectedIndex >= 0) {
        overlay.push({ i: rig.selectedIndex, color: ACCENT_RGB, intensity: 1 });
      }
      if (rig.hoverIndex >= 0 && rig.hoverIndex !== rig.selectedIndex) {
        overlay.push({ i: rig.hoverIndex, color: ACCENT_RGB, intensity: 0.65 });
      }

      renderer.render(camera, lod, overlay);
      if (rig.data && rig.grid) {
        labels.render(
          camera,
          lod,
          rig.data,
          rig.grid,
          Math.min(window.devicePixelRatio || 1, 2),
        );
      }

      /* fps over a 500ms window */
      frames++;
      if (t - fpsWindowStart >= 500) {
        const fps = Math.round((frames * 1000) / (t - fpsWindowStart));
        frames = 0;
        fpsWindowStart = t;
        setStats((s) =>
          s.fps === fps && s.lod === lod.level ? s : { ...s, fps, lod: lod.level },
        );
      }
      raf = requestAnimationFrame(frame);
    };
    raf = requestAnimationFrame(frame);

    return () => {
      disposed = true;
      cancelAnimationFrame(raf);
      ro.disconnect();
      container.removeEventListener("pointerdown", onPointerDown);
      container.removeEventListener("pointermove", onPointerMove);
      container.removeEventListener("pointerup", onPointerUp);
      container.removeEventListener("pointerleave", onLeave);
      container.removeEventListener("wheel", onWheel);
      renderer.destroy();
      rigRef.current = null;
    };
  }, []);

  /* ---- data：跟随 selectedRepoId / ego 状态加载，可取消 ---- */
  useEffect(() => {
    const rig = rigRef.current;
    if (!rig) return;
    /* repo 切换时退出旧 repo 的 ego 模式（effect 会以 ego=null 重跑） */
    if (ego && ego.repoId !== repoId) {
      setEgo(null);
      return;
    }
    let cancelled = false;
    const ctrl = new AbortController();
    setLoading(true);
    setPanel(null);
    rig.selectedIndex = -1;

    const run = async () => {
      if (ego) {
        const data = await loadEgoGraph(
          repoId,
          ego.id,
          EGO_DEPTH,
          ctrl.signal,
        ).catch(() => null);
        if (cancelled) return;
        if (data) {
          rig.applyData(data);
          rig.centerIndex = 0; /* 合同：ego 中心节点 i=0 */
          setLive(true);
          setEgoError(null);
          setLoading(false);
        } else {
          /* 旧后端无 /api/graph/ego —— 留在全图并提示 */
          setEgoError("当前后端不支持以节点为中心的子图（需更新 aka serve）");
          setEgo(null);
        }
      } else {
        const data = await loadRealGraph(repoId, ctrl.signal).catch(() => null);
        if (cancelled) return;
        rig.centerIndex = -1;
        if (data) {
          rig.applyData(data);
          setLive(true);
        } else {
          rig.applyData(generateDemoGraph());
          setLive(false);
        }
        setLoading(false);
      }
    };
    void run();
    return () => {
      cancelled = true;
      ctrl.abort();
    };
  }, [repoId, ego]);

  /* ---- 节点点击 → 面板基础信息 ---- */
  onSelectRef.current = (i: number) => {
    const rig = rigRef.current;
    if (!rig?.data || i < 0) {
      setPanel(null);
      return;
    }
    const d = rig.data;
    setPanel({
      index: i,
      id: d.id(i),
      name: d.name(i),
      label: d.classNames[d.classes[i]] ?? "Node",
      file: d.file(i),
    });
  };

  /* ---- 面板详情：GET /api/node，404/离线优雅降级 ---- */
  useEffect(() => {
    if (!panel) {
      setDetail(null);
      setDetailState("idle");
      return;
    }
    if (!live) {
      setDetail(null);
      setDetailState("degraded");
      setDegradeNote("离线演示数据——启动 aka serve 后可查看节点详情");
      return;
    }
    let stale = false;
    setDetail(null);
    setDetailState("loading");
    void fetchNodeDetail(repoId, panel.id).then((res) => {
      if (stale) return;
      if (res.state === "ok") {
        setDetail(res.detail);
        setDetailState("ok");
      } else {
        setDetailState("degraded");
        setDegradeNote(
          res.state === "unsupported"
            ? "详情需更新后端（/api/node 未实现）"
            : "节点详情获取失败——后端连接异常",
        );
      }
    });
    return () => {
      stale = true;
    };
  }, [panel, live, repoId]);

  const closePanel = () => {
    const rig = rigRef.current;
    if (rig) rig.selectedIndex = -1;
    setPanel(null);
  };

  const centerOnPanelNode = () => {
    if (!panel) return;
    setEgoError(null);
    setEgo({ repoId, id: panel.id, name: panel.name });
  };

  const openInSymbolView = () => {
    if (!panel) return;
    setQuery(panel.name);
    setView("symbol");
  };

  const zoom = (factor: number) => {
    const rig = rigRef.current;
    if (!rig) return;
    rig.camera.zoomAt(rig.camera.width / 2, rig.camera.height / 2, factor);
  };
  const fit = () => {
    const rig = rigRef.current;
    if (!rig?.data) return;
    rig.camera.fitBounds(rig.data.bounds);
  };

  const file = detail?.file || panel?.file || "";
  const line = detail?.line ?? 0;

  return (
    <div
      ref={containerRef}
      className="relative h-full w-full cursor-grab touch-none overflow-hidden active:cursor-grabbing"
      data-testid="graph-view"
    >
      <canvas ref={glRef} className="absolute inset-0 h-full w-full" />
      <canvas
        ref={labelRef}
        className="pointer-events-none absolute inset-0 h-full w-full"
      />

      {/* repo chip — top left */}
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ ...spring, delay: 0.05 }}
        className="glass absolute left-4 top-4 z-10 flex items-center gap-2 px-3.5 py-2"
        data-graph-ui
        data-testid="graph-repo-chip"
      >
        <span className="text-[12.5px] font-semibold text-ink">{repoId}</span>
        <span
          className="rounded-[6px] px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide"
          style={{
            color: live ? "#2563c9" : "#b25c0e",
            background: live
              ? "rgba(46,124,246,0.1)"
              : "rgba(246,166,35,0.12)",
          }}
        >
          {live ? "live" : "demo"}
        </span>
      </motion.div>

      {/* ego breadcrumb — top center */}
      <AnimatePresence>
        {ego && !loading && (
          <motion.div
            initial={{ opacity: 0, y: -8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -8 }}
            transition={spring}
            className="glass absolute left-1/2 top-4 z-10 flex -translate-x-1/2 items-center gap-2 py-1.5 pl-1.5 pr-3.5"
            data-graph-ui
            data-testid="ego-breadcrumb"
          >
            <button
              onClick={() => {
                setEgoError(null);
                setEgo(null);
              }}
              className="focus-ring flex items-center gap-1.5 rounded-[8px] px-2.5 py-1 text-[12px] font-medium text-[#2e7cf6] transition-colors duration-150 ease-out hover:bg-[rgba(46,124,246,0.08)]"
            >
              <span aria-hidden>←</span> 返回全图
            </button>
            <span className="h-3.5 w-px bg-[rgba(15,23,42,0.1)]" />
            <span className="text-[12px] text-ink-2">
              以{" "}
              <span className="mono font-semibold text-ink">{ego.name}</span>{" "}
              为中心 · 深度 {EGO_DEPTH}
            </span>
          </motion.div>
        )}
      </AnimatePresence>

      {/* ego unsupported / error note */}
      <AnimatePresence>
        {egoError && (
          <motion.div
            initial={{ opacity: 0, y: -8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={spring}
            className="glass absolute left-1/2 top-4 z-10 flex -translate-x-1/2 items-center gap-2.5 px-3.5 py-2"
            data-graph-ui
            data-testid="ego-error"
          >
            <span className="h-1.5 w-1.5 flex-none rounded-full bg-[#ff3b30]" />
            <span className="text-[12px] text-ink-2">{egoError}</span>
            <button
              onClick={() => setEgoError(null)}
              aria-label="Dismiss"
              className="focus-ring rounded-[6px] px-1 text-[13px] leading-none text-ink-3 hover:text-ink"
            >
              ×
            </button>
          </motion.div>
        )}
      </AnimatePresence>

      {/* loading card */}
      <AnimatePresence>
        {loading && (
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={spring}
            className="glass-panel absolute left-1/2 top-1/2 z-10 flex -translate-x-1/2 -translate-y-1/2 items-center gap-3 px-6 py-4"
            data-testid="graph-loading"
          >
            <span
              className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-[rgba(46,124,246,0.25)]"
              style={{ borderTopColor: "#2e7cf6" }}
            />
            <span className="text-[13px] font-medium text-ink-2">
              {ego ? (
                <>
                  正在加载{" "}
                  <span className="mono font-semibold">{ego.name}</span> 子图…
                </>
              ) : (
                <>
                  正在加载 <span className="mono font-semibold">{repoId}</span>{" "}
                  图谱…
                </>
              )}
            </span>
          </motion.div>
        )}
      </AnimatePresence>

      {/* hover tooltip */}
      <AnimatePresence>
        {hover && !panel && (
          <motion.div
            initial={{ opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.12, ease: "easeOut" }}
            className="glass pointer-events-none absolute z-20 px-3 py-2"
            style={{
              left: Math.min(
                hover.x + 14,
                Math.max(0, (containerRef.current?.clientWidth ?? 600) - 240),
              ),
              top: hover.y + 14,
              maxWidth: 260,
            }}
          >
            <div className="flex items-center gap-2">
              <span className="mono truncate text-[12px] font-semibold text-ink">
                {hover.name}
              </span>
              <span className={`badge ${hover.label}`}>{hover.label}</span>
            </div>
            <div className="mono mt-1 truncate text-[10.5px] text-ink-3">
              {hover.file}
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      {/* node detail panel — right side */}
      <AnimatePresence>
        {panel && (
          <motion.div
            key={`${repoId}:${panel.id}`}
            initial={{ opacity: 0, x: 24 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: 24 }}
            transition={spring}
            className="glass-panel absolute right-4 top-4 z-30 flex w-[320px] max-w-[calc(100%-2rem)] flex-col overflow-hidden"
            style={{ maxHeight: "calc(100% - 7rem)" }}
            data-graph-ui
            data-testid="node-detail-panel"
          >
            {/* header */}
            <div className="flex items-start gap-2 px-4 pb-2 pt-3.5">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="mono truncate text-[13.5px] font-semibold text-ink">
                    {panel.name}
                  </span>
                  <span className={`badge ${detail?.label ?? panel.label}`}>
                    {detail?.label ?? panel.label}
                  </span>
                </div>
                {(file || line > 0) && (
                  <div className="mono mt-1 truncate text-[11px] text-ink-3">
                    {file}
                    {line > 0 ? `:${line}` : ""}
                    {detail && detail.end_line > detail.line
                      ? `–${detail.end_line}`
                      : ""}
                  </div>
                )}
              </div>
              <button
                onClick={closePanel}
                aria-label="Close detail panel"
                className="focus-ring -mr-1 flex h-6 w-6 flex-none items-center justify-center rounded-[7px] text-[15px] leading-none text-ink-3 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)] hover:text-ink"
              >
                ×
              </button>
            </div>

            {/* body */}
            <div className="scroll-area min-h-0 flex-1 px-4 pb-3">
              {detailState === "loading" && (
                <div className="flex items-center gap-2 py-3 text-[12px] text-ink-3">
                  <span
                    className="h-3 w-3 animate-spin rounded-full border-2 border-[rgba(46,124,246,0.25)]"
                    style={{ borderTopColor: "#2e7cf6" }}
                  />
                  加载详情…
                </div>
              )}

              {detailState === "degraded" && (
                <div
                  className="my-2 rounded-[10px] px-3 py-2 text-[11.5px]"
                  style={{
                    background: "rgba(246,166,35,0.1)",
                    color: "#8a5a10",
                  }}
                  data-testid="detail-degraded"
                >
                  {degradeNote}
                </div>
              )}

              {detail && (
                <>
                  {/* degree */}
                  <div className="mb-3 mt-1 grid grid-cols-3 gap-1.5">
                    <DegreeStat label="callers" value={detail.degree.callers} />
                    <DegreeStat label="callees" value={detail.degree.callees} />
                    <DegreeStat label="refs" value={detail.degree.refs} />
                  </div>

                  {/* properties */}
                  {Object.keys(detail.properties ?? {}).length > 0 && (
                    <div className="mb-1">
                      <div className="mb-1.5 text-[10.5px] font-semibold uppercase tracking-[0.08em] text-ink-3">
                        Properties
                      </div>
                      <div
                        className="overflow-hidden rounded-[10px]"
                        style={{
                          boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.07)",
                        }}
                      >
                        {Object.entries(detail.properties).map(([k, v], i) => (
                          <PropRow key={k} k={k} v={v} alt={i % 2 === 1} />
                        ))}
                      </div>
                    </div>
                  )}
                </>
              )}
            </div>

            {/* actions */}
            <div className="flex flex-col gap-1.5 border-t border-[rgba(15,23,42,0.06)] px-4 py-3">
              <button
                onClick={centerOnPanelNode}
                className="btn-primary focus-ring px-3 py-2 text-[12.5px] font-semibold"
                data-testid="center-on-node"
              >
                以此为中心
              </button>
              <button
                onClick={openInSymbolView}
                className="focus-ring rounded-[10px] px-3 py-2 text-[12.5px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)] hover:text-ink"
                style={{ boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.1)" }}
                data-testid="open-in-symbol"
              >
                在 Symbol 视图打开
              </button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      {/* zoom controls — bottom left */}
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ ...spring, delay: 0.08 }}
        className="glass absolute bottom-4 left-4 z-10 flex flex-col overflow-hidden"
      >
        <CtrlButton label="Zoom in" onClick={() => zoom(1.7)}>
          <PlusIcon />
        </CtrlButton>
        <Divider />
        <CtrlButton label="Zoom out" onClick={() => zoom(1 / 1.7)}>
          <MinusIcon />
        </CtrlButton>
        <Divider />
        <CtrlButton label="Fit graph" onClick={fit}>
          <FitIcon />
        </CtrlButton>
      </motion.div>

      {/* stats badge — bottom right */}
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ ...spring, delay: 0.1 }}
        className="glass tabular absolute bottom-4 right-4 z-10 flex items-center gap-3 px-3.5 py-2 text-[11.5px]"
        data-testid="graph-stats"
      >
        <span className="flex items-center gap-1.5">
          <span
            className="h-1.5 w-1.5 rounded-full"
            style={{
              background: stats.fps >= 50 || loading ? "#34c759" : "#f6a623",
            }}
          />
          <span className="font-semibold text-ink" data-testid="fps-value">
            {stats.fps}
          </span>
          <span className="text-ink-3">fps</span>
        </span>
        <span className="text-ink-3">
          <span className="font-medium text-ink-2" data-testid="node-count">
            {formatCount(stats.nodes)}
          </span>{" "}
          nodes
        </span>
        <span className="text-ink-3">
          <span className="font-medium text-ink-2" data-testid="edge-count">
            {formatCount(stats.edges)}
          </span>{" "}
          edges
        </span>
        <span
          className="rounded-[6px] px-1.5 py-0.5 text-[10.5px] font-semibold uppercase tracking-wide text-ink-2"
          style={{ background: "rgba(15,23,42,0.05)" }}
          data-testid="lod-level"
        >
          {LOD_NAMES[stats.lod]}
        </span>
      </motion.div>
    </div>
  );
}

function DegreeStat({ label, value }: { label: string; value: number }) {
  return (
    <div
      className="tabular flex flex-col items-center rounded-[10px] px-2 py-1.5"
      style={{ background: "rgba(15,23,42,0.04)" }}
    >
      <span className="text-[14px] font-semibold text-ink">
        {formatCount(value)}
      </span>
      <span className="text-[10px] text-ink-3">{label}</span>
    </div>
  );
}

const PROP_TRUNCATE = 64;

function PropRow({ k, v, alt }: { k: string; v: unknown; alt: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const text = typeof v === "string" ? v : JSON.stringify(v);
  const long = text.length > PROP_TRUNCATE;
  const shown = expanded || !long ? text : `${text.slice(0, PROP_TRUNCATE)}…`;
  return (
    <div
      className="flex items-start gap-2 px-2.5 py-1.5"
      style={{ background: alt ? "rgba(15,23,42,0.025)" : "transparent" }}
    >
      <span className="mono w-[88px] flex-none truncate pt-px text-[10.5px] text-ink-3">
        {k}
      </span>
      <span
        className={`mono min-w-0 flex-1 text-[11px] text-ink-2 ${
          long ? "cursor-pointer" : ""
        } ${expanded ? "break-all" : "truncate"}`}
        title={long && !expanded ? "点击展开" : undefined}
        onClick={long ? () => setExpanded((x) => !x) : undefined}
      >
        {shown}
      </span>
    </div>
  );
}

function pickAt(rig: Rig, x: number, y: number): number {
  if (!rig.grid || !rig.data) return -1;
  const wx = rig.camera.screenToWorldX(x);
  const wy = rig.camera.screenToWorldY(y);
  const radius = 10 / rig.camera.k; /* ~10px hit area */
  return rig.grid.pick(wx, wy, radius);
}

function formatCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}K`;
  return String(n);
}

function CtrlButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick(): void;
  children: React.ReactNode;
}) {
  return (
    <button
      aria-label={label}
      title={label}
      onClick={onClick}
      className="focus-ring flex h-9 w-9 items-center justify-center text-ink-2 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)] hover:text-ink"
    >
      {children}
    </button>
  );
}

function Divider() {
  return <div className="mx-1.5 h-px bg-[rgba(15,23,42,0.07)]" />;
}

function PlusIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
      <path
        d="M12 5v14M5 12h14"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}

function MinusIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
      <path
        d="M5 12h14"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}

function FitIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
      <path
        d="M9 4H5a1 1 0 0 0-1 1v4M15 4h4a1 1 0 0 1 1 1v4M9 20H5a1 1 0 0 1-1-1v-4M15 20h4a1 1 0 0 0 1-1v-4"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
