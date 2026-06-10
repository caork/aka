/**
 * Uniform spatial grid over world space — CPU-side hover/click picking and
 * viewport label queries. Built once per dataset with a counting sort,
 * queries are O(cells touched).
 */

import type { Bounds } from "./format";

export class SpatialGrid {
  private cellSize: number;
  private cols: number;
  private rows: number;
  private minX: number;
  private minY: number;
  private cellStart: Uint32Array;
  private indices: Uint32Array;
  private positions: Float32Array;

  constructor(positions: Float32Array, count: number, bounds: Bounds) {
    this.positions = positions;
    const w = Math.max(1e-6, bounds.maxX - bounds.minX);
    const h = Math.max(1e-6, bounds.maxY - bounds.minY);
    /* aim for ~8-16 nodes per cell */
    const targetCells = Math.max(1, Math.ceil(count / 12));
    const aspect = w / h;
    this.cols = Math.max(1, Math.round(Math.sqrt(targetCells * aspect)));
    this.rows = Math.max(1, Math.ceil(targetCells / this.cols));
    this.cellSize = Math.max(w / this.cols, h / this.rows);
    this.cols = Math.max(1, Math.ceil(w / this.cellSize));
    this.rows = Math.max(1, Math.ceil(h / this.cellSize));
    this.minX = bounds.minX;
    this.minY = bounds.minY;

    const nCells = this.cols * this.rows;
    const counts = new Uint32Array(nCells + 1);
    const cellOf = new Uint32Array(count);
    for (let i = 0; i < count; i++) {
      const cx = clampInt(((positions[i * 2] - this.minX) / this.cellSize) | 0, 0, this.cols - 1);
      const cy = clampInt(((positions[i * 2 + 1] - this.minY) / this.cellSize) | 0, 0, this.rows - 1);
      const cell = cy * this.cols + cx;
      cellOf[i] = cell;
      counts[cell + 1]++;
    }
    for (let c = 0; c < nCells; c++) counts[c + 1] += counts[c];
    this.cellStart = counts;
    this.indices = new Uint32Array(count);
    const cursor = counts.slice(0, nCells);
    for (let i = 0; i < count; i++) {
      this.indices[cursor[cellOf[i]]++] = i;
    }
  }

  /** nearest node to (x, y) within radius (world units), or -1 */
  pick(x: number, y: number, radius: number): number {
    const c0x = clampInt(((x - radius - this.minX) / this.cellSize) | 0, 0, this.cols - 1);
    const c1x = clampInt(((x + radius - this.minX) / this.cellSize) | 0, 0, this.cols - 1);
    const c0y = clampInt(((y - radius - this.minY) / this.cellSize) | 0, 0, this.rows - 1);
    const c1y = clampInt(((y + radius - this.minY) / this.cellSize) | 0, 0, this.rows - 1);
    let best = -1;
    let bestD2 = radius * radius;
    for (let cy = c0y; cy <= c1y; cy++) {
      for (let cx = c0x; cx <= c1x; cx++) {
        const cell = cy * this.cols + cx;
        const s = this.cellStart[cell];
        const e = this.cellStart[cell + 1];
        for (let p = s; p < e; p++) {
          const i = this.indices[p];
          const dx = this.positions[i * 2] - x;
          const dy = this.positions[i * 2 + 1] - y;
          const d2 = dx * dx + dy * dy;
          if (d2 < bestD2) {
            bestD2 = d2;
            best = i;
          }
        }
      }
    }
    return best;
  }

  /**
   * Visit nodes inside the world-space rect. Returns false (and stops) if
   * the rect spans more than `maxCells` cells — caller should treat that as
   * "zoomed out too far for per-node work".
   */
  forEachInRect(
    minX: number,
    minY: number,
    maxX: number,
    maxY: number,
    maxCells: number,
    visit: (i: number) => void,
  ): boolean {
    const c0x = clampInt(((minX - this.minX) / this.cellSize) | 0, 0, this.cols - 1);
    const c1x = clampInt(((maxX - this.minX) / this.cellSize) | 0, 0, this.cols - 1);
    const c0y = clampInt(((minY - this.minY) / this.cellSize) | 0, 0, this.rows - 1);
    const c1y = clampInt(((maxY - this.minY) / this.cellSize) | 0, 0, this.rows - 1);
    if ((c1x - c0x + 1) * (c1y - c0y + 1) > maxCells) return false;
    for (let cy = c0y; cy <= c1y; cy++) {
      for (let cx = c0x; cx <= c1x; cx++) {
        const cell = cy * this.cols + cx;
        const s = this.cellStart[cell];
        const e = this.cellStart[cell + 1];
        for (let p = s; p < e; p++) {
          const i = this.indices[p];
          const x = this.positions[i * 2];
          const y = this.positions[i * 2 + 1];
          if (x >= minX && x <= maxX && y >= minY && y <= maxY) visit(i);
        }
      }
    }
    return true;
  }
}

function clampInt(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}
