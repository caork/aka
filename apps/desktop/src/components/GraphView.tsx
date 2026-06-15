import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { Camera } from "../graph/camera";
import type { GraphData } from "../graph/format";
import { loadEgoGraph, loadRealGraph } from "../graph/source";
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
import { useAppStore } from "../store";
import IndexingPanel from "./IndexingPanel";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

const ACCENT_RGB = hexToRgb(ACCENT);
const BEACON_RGB = hexToRgb(BEACON);
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

interface EgoState {
  repoId: string;
  id: string;
  name: string;
}

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
  const repo = useAppStore(
    (s) => s.repos.find((r) => r.id === s.selectedRepoId) ?? null,
  );
  const detailTarget = useAppStore((s) => s.detailTarget);
  const openDetail = useAppStore((s) => s.openDetail);
  const closeDetail = useAppStore((s) => s.closeDetail);
  const egoRequest = useAppStore((s) => s.egoRequest);
  const clearEgoRequest = useAppStore((s) => s.clearEgoRequest);
  const focusRequest = useAppStore((s) => s.focusRequest);
  const clearFocusRequest = useAppStore((s) => s.clearFocusRequest);
  useAppStore((s) => s.resolvedTheme);

  const [stats, setStats] = useState<Stats>({ fps: 0, lod: 0, nodes: 0, edges: 0 });
  const [hover, setHover] = useState<HoverInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [emptyReason, setEmptyReason] = useState<"none" | "missing" | "unavailable">("none");
  const [ego, setEgo] = useState<EgoState | null>(null);
  const [egoError, setEgoError] = useState<string | null>(null);

  /** 仓库总节点数（来自 /api/repos stats），用于徽章 "已渲染 N / 总数" */
  const totalNodes = repo?.symbols ?? 0;
  /** 全图渲染预算：优先用仓库总符号数，避免回退到后端默认 LOD 截断。 */
  const renderBudget = totalNodes > 0 ? totalNodes : (repo?.renderMaxNodes ?? null);
  const repoPending = repo?.status === "indexing" || repo?.status === "failed";

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
      if (rig.data && rig.grid) {
        const lod = computeLod(camera.zoomLevel);
        renderer.render(camera, lod, [], useAppStore.getState().resolvedTheme);
        labels.render(
          camera,
          lod,
          rig.data,
          rig.grid,
          dpr,
          useAppStore.getState().resolvedTheme,
        );
      }
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

      renderer.render(camera, lod, overlay, useAppStore.getState().resolvedTheme);
      if (rig.data && rig.grid) {
        labels.render(
          camera,
          lod,
          rig.data,
          rig.grid,
          Math.min(window.devicePixelRatio || 1, 2),
          useAppStore.getState().resolvedTheme,
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
    if (!repoId || !repo) {
      rig.renderer.clearData();
      rig.labels.clear();
      rig.data = null;
      rig.grid = null;
      rig.beacons = [];
      rig.centerIndex = -1;
      rig.selectedIndex = -1;
      rig.hoverIndex = -1;
      setHover(null);
      setStats((s) => ({ ...s, nodes: 0, edges: 0 }));
      setLoading(false);
      setEmptyReason("missing");
      return;
    }
    if (repoPending) {
      rig.renderer.clearData();
      rig.labels.clear();
      rig.data = null;
      rig.grid = null;
      rig.beacons = [];
      rig.centerIndex = -1;
      rig.selectedIndex = -1;
      rig.hoverIndex = -1;
      setHover(null);
      setLoading(false);
      setEmptyReason("none");
      setEgo(null);
      setEgoError(null);
      setStats((s) => ({ ...s, nodes: 0, edges: 0 }));
      return;
    }
    /* repo 切换时退出旧 repo 的 ego 模式（effect 会以 ego=null 重跑） */
    if (ego && ego.repoId !== repoId) {
      setEgo(null);
      return;
    }
    let cancelled = false;
    const ctrl = new AbortController();
    setLoading(true);
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
          setEmptyReason("none");
          setEgoError(null);
          setLoading(false);
        } else {
          /* 旧后端无 /api/graph/ego —— 留在全图并提示 */
          setEgoError("当前后端不支持以节点为中心的子图（需更新 aka serve）");
          setEgo(null);
        }
      } else {
        const data = await loadRealGraph(repoId, renderBudget, ctrl.signal).catch(
          () => null,
        );
        if (cancelled) return;
        rig.centerIndex = -1;
        if (data) {
          rig.applyData(data);
          setEmptyReason("none");
        } else {
          rig.renderer.clearData();
          rig.labels.clear();
          rig.data = null;
          rig.grid = null;
          rig.beacons = [];
          rig.centerIndex = -1;
          rig.selectedIndex = -1;
          rig.hoverIndex = -1;
          setHover(null);
          setStats((s) => ({ ...s, nodes: 0, edges: 0 }));
          setEmptyReason("unavailable");
        }
        setLoading(false);
      }
    };
    void run();
    return () => {
      cancelled = true;
      ctrl.abort();
    };
  }, [repoId, repo, repoPending, ego, renderBudget]);

  /* ---- DetailPanel「Ego 视图」发来的下钻请求 ---- */
  useEffect(() => {
    if (!egoRequest) return;
    setEgoError(null);
    setEgo({ repoId, id: egoRequest.id, name: egoRequest.name });
    clearEgoRequest();
  }, [egoRequest, repoId, clearEgoRequest]);

  /* ---- CodeView「在 Graph 中定位」发来的请求 ----
     等当前数据加载完成后消费：
     · 在已加载图中找到节点 → 相机平滑动画到该节点（只写 target，不动
       fitK/LOD 基准），选中（发光蓝）并打开 DetailPanel；
     · 没找到（不在当前 ego 子图）→ 回退走与 requestEgo
       相同的 ego 加载路径，以该节点为中心。 */
  useEffect(() => {
    if (!focusRequest || loading) return;
    const rig = rigRef.current;
    if (!rig?.data) return;
    const d = rig.data;
    let found = -1;
    for (let i = 0; i < d.count; i++) {
      if (d.id(i) === focusRequest.id) {
        found = i;
        break;
      }
    }
    clearFocusRequest();
    if (found >= 0) {
      rig.selectedIndex = found;
      focusCameraOn(
        rig.camera,
        d.positions[found * 2],
        d.positions[found * 2 + 1],
      );
      openDetail({
        id: d.id(found),
        name: d.name(found),
        label: d.classNames[d.classes[found]] ?? "Node",
        file: d.file(found),
        line: 0 /* LOD 快照不带行号——DetailPanel 经 /api/node 补全 */,
      });
    } else {
      setEgoError(null);
      setEgo({ repoId, id: focusRequest.id, name: focusRequest.name });
    }
  }, [focusRequest, loading, repoId, clearFocusRequest, openDetail]);

  /* ---- 节点点击 → 打开共用 DetailPanel（点空白处关闭） ---- */
  onSelectRef.current = (i: number) => {
    const rig = rigRef.current;
    if (!rig?.data || i < 0) {
      closeDetail();
      return;
    }
    const d = rig.data;
    if (d.id(i).startsWith("cluster:")) {
      closeDetail();
      return;
    }
    openDetail({
      id: d.id(i),
      name: d.name(i),
      label: d.classNames[d.classes[i]] ?? "Node",
      file: d.file(i),
      line: 0 /* LOD 快照不带行号——DetailPanel 经 /api/node 补全 */,
    });
  };

  /* ---- 面板关闭 / 目标变化（面板内点关系条目）时同步选中高亮 ---- */
  useEffect(() => {
    const rig = rigRef.current;
    if (!rig) return;
    if (!detailTarget) {
      rig.selectedIndex = -1;
      return;
    }
    if (
      rig.selectedIndex >= 0 &&
      rig.data &&
      rig.data.id(rig.selectedIndex) !== detailTarget.id
    ) {
      rig.selectedIndex = -1;
    }
  }, [detailTarget]);

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

  return (
    <div
      ref={containerRef}
      className="relative h-full w-full cursor-grab touch-none overflow-hidden active:cursor-grabbing"
      data-testid="graph-view"
      data-detail-manage /* 画布自己管理 DetailPanel 的开/关，拖拽不误关面板 */
    >
      <canvas ref={glRef} className="absolute inset-0 h-full w-full" />
      <canvas
        ref={labelRef}
        className="pointer-events-none absolute inset-0 h-full w-full"
      />
      <div
        aria-hidden
        className="graph-top-glass pointer-events-none absolute inset-x-0 top-0 z-[5]"
      />

      <div
        className="graph-top-sheen pointer-events-none absolute inset-x-0 top-0 z-[5]"
        aria-hidden
      />

      {/* ego breadcrumb — top center */}
      <AnimatePresence>
        {ego && !loading && (
          <motion.div
            initial={{ opacity: 0, y: -8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -8 }}
            transition={spring}
            className="glass absolute left-1/2 top-4 z-30 flex -translate-x-1/2 items-center gap-2 py-1.5 pl-1.5 pr-3.5"
            data-graph-ui
            data-testid="ego-breadcrumb"
          >
            <button
              onClick={() => {
                setEgoError(null);
                setEgo(null);
              }}
              className="focus-ring flex items-center gap-1.5 rounded-[8px] px-2.5 py-1 text-[12px] font-medium text-[var(--accent)] transition-colors duration-150 ease-out hover:bg-[var(--accent-fill)]"
            >
              <span aria-hidden>←</span> 返回全图
            </button>
            <span
              className="h-3.5 w-px"
              style={{ background: "var(--hairline-strong)" }}
            />
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
            className="glass absolute left-1/2 top-4 z-30 flex -translate-x-1/2 items-center gap-2.5 px-3.5 py-2"
            data-graph-ui
            data-testid="ego-error"
          >
            <span
              className="h-1.5 w-1.5 flex-none rounded-full"
              style={{ background: "var(--danger)" }}
            />
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
        {!loading && emptyReason !== "none" && (
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={spring}
            className="glass-panel absolute left-1/2 top-1/2 z-10 max-w-[360px] -translate-x-1/2 -translate-y-1/2 px-6 py-5 text-center"
            data-testid="graph-empty"
          >
            <div className="text-[14px] font-semibold text-ink">
              {emptyReason === "missing" ? "No repositories" : "Graph unavailable"}
            </div>
            <div className="mt-1.5 text-[12px] leading-relaxed text-ink-3">
              {emptyReason === "missing"
                ? "Import a repository to view its graph."
                : "This repository does not have graph data yet."}
            </div>
          </motion.div>
        )}
        {repoPending && repo && (
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={spring}
            className="absolute inset-0 z-10"
            data-graph-ui
            data-testid="graph-indexing-panel"
          >
            <IndexingPanel repo={repo} />
          </motion.div>
        )}
        {loading && repoId && (
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
              style={{ borderTopColor: "var(--accent)" }}
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
        {hover && !detailTarget && (
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

      {/* zoom controls — bottom left, shifted to clear the repo button */}
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ ...spring, delay: 0.08 }}
        className="glass absolute bottom-4 left-[68px] z-10 flex flex-col overflow-hidden"
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
              background:
                stats.fps >= 50 || loading ? "var(--success)" : "var(--beacon)",
            }}
          />
          <span className="font-semibold text-ink" data-testid="fps-value">
            {stats.fps}
          </span>
          <span className="text-ink-3">fps</span>
        </span>
        <span
          className="text-ink-3"
          title={
            totalNodes > 0
              ? `已渲染 ${stats.nodes.toLocaleString()} / 仓库共 ${totalNodes.toLocaleString()} 节点`
              : `已渲染 ${stats.nodes.toLocaleString()} 节点`
          }
        >
          <span className="font-medium text-ink-2" data-testid="node-count">
            {formatCount(stats.nodes)}
          </span>
          {totalNodes > 0 && (
            <span data-testid="node-total"> / {formatCount(totalNodes)}</span>
          )}{" "}
          nodes
        </span>
        <span className="text-ink-3">
          <span className="font-medium text-ink-2" data-testid="edge-count">
            {formatCount(stats.edges)}
          </span>{" "}
          edges
        </span>
      </motion.div>
    </div>
  );
}

/**
 * 相机平滑动画到世界坐标 (x, y)，缩放到能看清单节点的层级。
 * 只写 target（update 的临界阻尼插值负责动画），刻意不走 fitBounds——
 * 那会用局部 bounds 重置 fitK，破坏 LOD 的全图缩放基准。
 */
function focusCameraOn(camera: Camera, x: number, y: number): void {
  /* zoomLevel = k / fitK ≥ 12 即 LOD "near"；16 留些余量。已更近则保持。 */
  const k = Math.min(camera.maxK, Math.max(camera.targetK, camera.fitK * 16));
  camera.targetK = k;
  camera.targetTx = camera.width / 2 - x * k;
  camera.targetTy = camera.height / 2 - y * k;
  camera.vx = 0;
  camera.vy = 0;
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
      className="themed-hover focus-ring flex h-9 w-9 items-center justify-center text-ink-2 transition-colors duration-150 ease-out hover:text-ink"
    >
      {children}
    </button>
  );
}

function Divider() {
  return <div className="mx-1.5 h-px" style={{ background: "var(--hairline)" }} />;
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
