/**
 * 2D camera: world -> screen is `screen = world * k + t`.
 *
 * All interaction writes to *target* values;每帧 `update(dt)` 用临界阻尼
 * 指数插值把当前值收敛到目标，保证 zoom / fit 丝滑。拖拽期间 pan 直写
 * current + target（1:1 跟手），松手后用速度做惯性滑动。
 *
 * Zoom 动画的锚点稳定性：k 在 log 空间插值（zoom 手感均匀），tx/ty 则按
 * k 的线性比例 `(newK - k) / (targetK - k)` 跟进——锚点不动要求
 * `tx(k) = px - w*k`（w 为光标下的世界坐标），即 tx 随 k 线性变化；
 * zoomAt 在 target 空间维护该约束，update 的线性比例插值保证中间帧
 * 始终留在这条锚点直线上，缩放过程中光标处画面严格不漂移。
 */

import type { Bounds } from "./format";

const STIFFNESS = 18; /* 1/s — convergence rate for the damped lerp */
const FRICTION = 5.2; /* 1/s — inertia decay */

export class Camera {
  /* current (rendered) state */
  k = 1;
  tx = 0;
  ty = 0;

  /* targets */
  targetK = 1;
  targetTx = 0;
  targetTy = 0;

  /* inertia velocity, px/s */
  vx = 0;
  vy = 0;

  /* viewport in css px */
  width = 1;
  height = 1;

  minK = 1e-4;
  maxK = 1e4;
  fitK = 1;

  setViewport(width: number, height: number) {
    this.width = Math.max(1, width);
    this.height = Math.max(1, height);
  }

  /** zoom level relative to "fit whole graph" — drives LOD */
  get zoomLevel(): number {
    return this.k / this.fitK;
  }

  screenToWorldX(px: number): number {
    return (px - this.tx) / this.k;
  }
  screenToWorldY(py: number): number {
    return (py - this.ty) / this.k;
  }
  worldToScreenX(wx: number): number {
    return wx * this.k + this.tx;
  }
  worldToScreenY(wy: number): number {
    return wy * this.k + this.ty;
  }

  /** 1:1 pan during drag — kills any pending animation */
  panBy(dx: number, dy: number) {
    this.tx += dx;
    this.ty += dy;
    this.targetTx = this.tx;
    this.targetTy = this.ty;
    this.targetK = this.k;
    this.vx = 0;
    this.vy = 0;
  }

  /** kick off inertia after a drag release, velocity in px/s */
  fling(vx: number, vy: number) {
    this.vx = vx;
    this.vy = vy;
  }

  stop() {
    this.vx = 0;
    this.vy = 0;
    this.targetTx = this.tx;
    this.targetTy = this.ty;
    this.targetK = this.k;
  }

  /** zoom keeping the world point under (px, py) fixed */
  zoomAt(px: number, py: number, factor: number) {
    const nextK = clamp(this.targetK * factor, this.minK, this.maxK);
    const applied = nextK / this.targetK;
    if (applied === 1) return;
    /* keep cursor-anchored point stable in *target* space */
    this.targetTx = px - (px - this.targetTx) * applied;
    this.targetTy = py - (py - this.targetTy) * applied;
    this.targetK = nextK;
    this.vx = 0;
    this.vy = 0;
  }

  /** compute the scale that fits the bounds, store as LOD reference */
  setFit(bounds: Bounds, pad = 0.92) {
    const w = Math.max(1e-6, bounds.maxX - bounds.minX);
    const h = Math.max(1e-6, bounds.maxY - bounds.minY);
    this.fitK = Math.min(this.width / w, this.height / h) * pad;
    this.minK = this.fitK * 0.35;
    this.maxK = this.fitK * 600;
  }

  /** animate (spring) to fit the bounds */
  fitBounds(bounds: Bounds, immediate = false) {
    this.setFit(bounds);
    const cx = (bounds.minX + bounds.maxX) / 2;
    const cy = (bounds.minY + bounds.maxY) / 2;
    this.targetK = this.fitK;
    this.targetTx = this.width / 2 - cx * this.fitK;
    this.targetTy = this.height / 2 - cy * this.fitK;
    this.vx = 0;
    this.vy = 0;
    if (immediate) {
      this.k = this.targetK;
      this.tx = this.targetTx;
      this.ty = this.targetTy;
    }
  }

  /** advance the damped interpolation; returns true while still moving */
  update(dt: number): boolean {
    /* inertia moves the pan target */
    if (Math.abs(this.vx) > 1 || Math.abs(this.vy) > 1) {
      this.targetTx += this.vx * dt;
      this.targetTy += this.vy * dt;
      const decay = Math.exp(-FRICTION * dt);
      this.vx *= decay;
      this.vy *= decay;
    } else {
      this.vx = 0;
      this.vy = 0;
    }

    const a = 1 - Math.exp(-STIFFNESS * dt);

    /* zoom interpolates in log space so it feels uniform */
    const logK = Math.log(this.k);
    const logT = Math.log(this.targetK);
    const dLog = logT - logK;
    let moving = false;
    if (Math.abs(dLog) > 1e-4) {
      /* k 在 log 空间插值（手感均匀），但 tx/ty 必须按 k 的**线性**比例
         跟进：锚点不动的约束是 tx(k) = px - w*k —— tx 随 k 线性变化。
         current 与 target 都落在同一条锚点直线上（zoomAt 在 target 空间
         维护该约束），线性比例插值让中间帧严格留在这条直线上，锚点
         世界点全程不漂移。之前用 log 比例插 tx/ty 会偏离直线 → 跳动。 */
      const newK = Math.exp(logK + dLog * a);
      const dK = this.targetK - this.k;
      const ratio = Math.abs(dK) < 1e-12 ? 1 : (newK - this.k) / dK;
      this.tx = this.tx + (this.targetTx - this.tx) * ratio;
      this.ty = this.ty + (this.targetTy - this.ty) * ratio;
      this.k = newK;
      moving = true;
    } else {
      this.k = this.targetK;
      const dx = this.targetTx - this.tx;
      const dy = this.targetTy - this.ty;
      if (Math.abs(dx) > 0.05 || Math.abs(dy) > 0.05) {
        this.tx += dx * a;
        this.ty += dy * a;
        moving = true;
      } else {
        this.tx = this.targetTx;
        this.ty = this.targetTy;
      }
    }
    return moving || this.vx !== 0 || this.vy !== 0;
  }
}

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}
