/**
 * Canvas2D label overlay.
 *
 * - mid LOD: cluster labels (top clusters by weight inside the viewport)
 *   fade in between z=3..4.5 and out again past z=14.
 * - near LOD: per-node labels for the top N (≤200) visible nodes by
 *   degree·size priority, drawn as glass-white pills.
 *
 * Individual labels fade in (120 ms) and fade out (200 ms) when they
 * appear or disappear due to pan, zoom, or de-overlap changes.
 */

import type { Camera } from "./camera";
import type { GraphData } from "./format";
import type { SpatialGrid } from "./grid";
import type { LodParams } from "./renderer";
import type { ResolvedTheme } from "../theme";

const MAX_NODE_LABELS = 200;
const FONT =
  '-apple-system, "SF Pro Text", Inter, sans-serif';
const FADE_IN_SPEED  = 1 / 120; // alpha/ms → 120 ms full fade-in
const FADE_OUT_SPEED = 1 / 200; // alpha/ms → 200 ms full fade-out

const LABEL_THEME = {
  light: {
    cluster: [71, 85, 105] as const,
    clusterShadow: [246, 248, 251] as const,
    pill: [255, 255, 255] as const,
    pillStroke: [15, 23, 42] as const,
    text: [15, 23, 42] as const,
  },
  dark: {
    cluster: [184, 196, 213] as const,
    clusterShadow: [16, 21, 29] as const,
    pill: [24, 31, 43] as const,
    pillStroke: [255, 255, 255] as const,
    text: [238, 244, 251] as const,
  },
};

interface Candidate {
  i: number;
  priority: number;
}

export class LabelOverlay {
  private ctx: CanvasRenderingContext2D;
  private canvas: HTMLCanvasElement;
  private candidates: Candidate[] = [];

  /** Per-label current alpha [0, 1] tracked across frames. */
  private nodeFade    = new Map<number, number>();
  private clusterFade = new Map<number, number>();
  /** Node labels selected last frame — drives placement hysteresis. */
  private prevNodeSel = new Set<number>();
  private prevTime    = 0;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d")!;
  }

  resize(cssWidth: number, cssHeight: number, dpr: number) {
    const w = Math.round(cssWidth * dpr);
    const h = Math.round(cssHeight * dpr);
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }

  render(
    camera: Camera,
    lod: LodParams,
    data: GraphData,
    grid: SpatialGrid,
    dpr: number,
    theme: ResolvedTheme = "light",
  ) {
    const now = performance.now();
    const dt  = this.prevTime === 0 ? 16 : Math.min(now - this.prevTime, 50);
    this.prevTime = now;

    const ctx = this.ctx;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, camera.width, camera.height);

    const clusterZoomAlpha =
      smoothstep(3, 4.5, lod.z) * (1 - smoothstep(14, 20, lod.z));
    if (clusterZoomAlpha > 0.01) {
      this.drawClusterLabels(camera, data, clusterZoomAlpha, dt, theme);
    } else {
      this.clusterFade.clear();
    }

    const nodeZoomAlpha = smoothstep(12, 16, lod.z);
    if (nodeZoomAlpha > 0.01) {
      this.drawNodeLabels(camera, lod, data, grid, nodeZoomAlpha, dt, theme);
    } else {
      this.nodeFade.clear();
      this.prevNodeSel.clear();
    }
  }

  private drawClusterLabels(
    camera: Camera,
    data: GraphData,
    zoomAlpha: number,
    dt: number,
    theme: ResolvedTheme,
  ) {
    const ctx = this.ctx;
    const colors = LABEL_THEME[theme];
    const pad = 60;

    /* Which clusters are visible and in the top-24 by weight */
    const selected = new Set<number>();
    data.clusterMeta
      .map((m, idx) => ({ m, idx }))
      .filter(({ m }) => {
        const sx = camera.worldToScreenX(m.x);
        const sy = camera.worldToScreenY(m.y);
        return (
          sx > -pad && sx < camera.width  + pad &&
          sy > -pad && sy < camera.height + pad
        );
      })
      .sort((a, b) => b.m.weight - a.m.weight)
      .slice(0, 24)
      .forEach(({ idx }) => selected.add(idx));

    /* Advance per-cluster alphas toward their targets */
    for (const idx of selected) {
      this.clusterFade.set(
        idx,
        Math.min(1, (this.clusterFade.get(idx) ?? 0) + dt * FADE_IN_SPEED),
      );
    }
    for (const [idx, a] of this.clusterFade) {
      if (!selected.has(idx)) {
        const next = a - dt * FADE_OUT_SPEED;
        if (next <= 0.001) this.clusterFade.delete(idx);
        else this.clusterFade.set(idx, next);
      }
    }

    /* Draw all clusters still in the fade map (includes fading-out ones) */
    ctx.font = `600 12px ${FONT}`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    for (const [idx, fade] of this.clusterFade) {
      const a = zoomAlpha * fade;
      if (a < 0.01) continue;
      const m  = data.clusterMeta[idx];
      const sx = camera.worldToScreenX(m.x);
      const sy = camera.worldToScreenY(m.y);
      ctx.fillStyle   = rgba(colors.cluster, 0.78 * a);
      ctx.shadowColor = rgba(colors.clusterShadow, 0.9 * a);
      ctx.shadowBlur  = 6;
      ctx.fillText(m.name, sx, sy);
    }
    ctx.shadowBlur = 0;
  }

  private drawNodeLabels(
    camera: Camera,
    lod: LodParams,
    data: GraphData,
    grid: SpatialGrid,
    zoomAlpha: number,
    dt: number,
    theme: ResolvedTheme,
  ) {
    const ctx  = this.ctx;
    const colors = LABEL_THEME[theme];
    const minX = camera.screenToWorldX(-20);
    const maxX = camera.screenToWorldX(camera.width  + 20);
    const minY = camera.screenToWorldY(-20);
    const maxY = camera.screenToWorldY(camera.height + 20);

    this.candidates.length = 0;
    const ok = grid.forEachInRect(minX, minY, maxX, maxY, 4096, (i) => {
      this.candidates.push({
        i,
        priority: data.degrees[i] * 4 + data.sizes[i],
      });
    });
    if (!ok) {
      this.nodeFade.clear();
      this.prevNodeSel.clear();
      return; /* viewport too wide for per-node labels */
    }

    /* Importance order. The index tie-break makes the ordering depend only on
       node identity, not on the grid's scan order — without it, ties resolve by
       spatial sweep direction, which made one screen edge (the bottom) win the
       de-overlap and reveal its labels first. */
    this.candidates.sort((a, b) => b.priority - a.priority || a.i - b.i);
    const n = Math.min(this.candidates.length, MAX_NODE_LABELS);

    ctx.font = `500 11px ${FONT}`;
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";

    /* --- De-overlap selection ---
     * Reserve the label's real pill box (≈2 rows tall) in a uniform screen
     * grid, then place candidates greedily. Placement runs in two passes:
     * labels shown last frame claim their slots first, so a steady pan/zoom
     * keeps them put instead of flip-flopping selected/unselected every frame
     * (the source of the flicker). New labels then fill whatever gaps remain. */
    const cell     = 14;
    const cols     = Math.ceil(camera.width / cell) + 1;
    const ph       = 17; /* pill height, matches the render pass */
    const occupied = new Set<number>();
    const selected = new Set<number>();

    const tryPlace = (i: number): void => {
      const sx = camera.worldToScreenX(data.positions[i * 2]);
      const sy = camera.worldToScreenY(data.positions[i * 2 + 1]);
      const rPx = Math.min(
        Math.max(data.sizes[i] * camera.k * 0.5, lod.minPx),
        lod.maxPx,
      );
      const tw = ctx.measureText(data.name(i)).width;
      const lx = sx + rPx + 6;
      const ly = sy;
      if (lx + tw > camera.width + 8 || ly < -8 || ly > camera.height + 8) return;

      const r0 = Math.floor((ly - ph / 2) / cell);
      const r1 = Math.floor((ly + ph / 2) / cell);
      const c0 = Math.max(0, Math.floor(lx / cell));
      const c1 = Math.min(cols - 1, Math.floor((lx + tw + 8) / cell));
      for (let r = r0; r <= r1; r++)
        for (let c = c0; c <= c1; c++)
          if (occupied.has(r * cols + c)) return;
      for (let r = r0; r <= r1; r++)
        for (let c = c0; c <= c1; c++) occupied.add(r * cols + c);
      selected.add(i);
    };

    for (let j = 0; j < n; j++) {
      const i = this.candidates[j].i;
      if (this.prevNodeSel.has(i)) tryPlace(i);
    }
    for (let j = 0; j < n; j++) {
      const i = this.candidates[j].i;
      if (!this.prevNodeSel.has(i)) tryPlace(i);
    }
    this.prevNodeSel = selected;

    /* --- Advance per-node alphas toward their targets --- */
    for (const i of selected) {
      this.nodeFade.set(
        i,
        Math.min(1, (this.nodeFade.get(i) ?? 0) + dt * FADE_IN_SPEED),
      );
    }
    for (const [i, a] of this.nodeFade) {
      if (!selected.has(i)) {
        const next = a - dt * FADE_OUT_SPEED;
        if (next <= 0.001) this.nodeFade.delete(i);
        else this.nodeFade.set(i, next);
      }
    }

    /* --- Render all labels in the map (fade-in and fade-out alike) ---
     * Sort ascending by alpha so fading-in labels paint on top of fading-out. */
    const entries = [...this.nodeFade.entries()].sort((a, b) => a[1] - b[1]);
    for (const [i, fade] of entries) {
      const a = zoomAlpha * fade;
      if (a < 0.01) continue;

      const sx = camera.worldToScreenX(data.positions[i * 2]);
      const sy = camera.worldToScreenY(data.positions[i * 2 + 1]);
      const rPx = Math.min(
        Math.max(data.sizes[i] * camera.k * 0.5, lod.minPx),
        lod.maxPx,
      );
      const label = data.name(i);
      const tw    = ctx.measureText(label).width;
      const lx    = sx + rPx + 6;
      const ly    = sy;

      /* glass pill */
      const pw = tw + 12;
      ctx.fillStyle   = rgba(colors.pill, (theme === "dark" ? 0.78 : 0.72) * a);
      ctx.strokeStyle = rgba(colors.pillStroke, (theme === "dark" ? 0.1 : 0.05) * a);
      ctx.lineWidth   = 1;
      roundRect(ctx, lx - 6, ly - ph / 2, pw, ph, 6);
      ctx.fill();
      ctx.stroke();
      ctx.fillStyle = rgba(colors.text, 0.82 * a);
      ctx.fillText(label, lx, ly + 0.5);
    }
  }
}

function rgba(rgb: readonly [number, number, number], alpha: number): string {
  return `rgba(${rgb[0]}, ${rgb[1]}, ${rgb[2]}, ${alpha})`;
}

function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y,     x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x,     y + h, r);
  ctx.arcTo(x,     y + h, x,     y,     r);
  ctx.arcTo(x,     y,     x + w, y,     r);
  ctx.closePath();
}

function smoothstep(lo: number, hi: number, v: number): number {
  const t = Math.min(1, Math.max(0, (v - lo) / (hi - lo)));
  return t * t * (3 - 2 * t);
}
