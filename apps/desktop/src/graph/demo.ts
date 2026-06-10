/**
 * Synthetic demo dataset — 500k nodes / 1M edges by default.
 *
 * Layout is static: clusters are placed on a global phyllotaxis spiral, and
 * nodes inside each cluster follow their own phyllotaxis (sunflower) packing,
 * which gives an even, organic distribution with O(1) generation per node.
 *
 * Node names / ids / file paths are synthesized lazily from the index so we
 * never hold 500k strings in memory.
 */

import type { Bounds, ClusterMeta, GraphData } from "./format";
import { computeClusterMeta, computeDegrees } from "./format";

const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

export const DEMO_CLASSES = [
  "Function",
  "Method",
  "Class",
  "Interface",
  "File",
  "Struct",
  "Enum",
  "Trait",
];

/* class distribution, cumulative */
const CLASS_CDF = [0.45, 0.65, 0.77, 0.82, 0.94, 0.97, 0.99, 1.0];

const WORDS_A = [
  "parse", "index", "query", "graph", "token", "merge", "flush", "watch",
  "render", "score", "embed", "chunk", "trace", "patch", "split", "cache",
];
const WORDS_B = [
  "engine", "store", "worker", "buffer", "stream", "cursor", "schema", "router",
  "filter", "loader", "writer", "reader", "mapper", "runner", "probe", "pool",
];
const MODULES = [
  "core", "search", "graph", "mcp", "server", "ingest", "lsp", "cli",
  "fs", "git", "ast", "vec", "net", "ui", "sync", "db",
];

function makeRng(seed: number) {
  let s = seed >>> 0 || 1;
  return () => {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    s >>>= 0;
    return s / 4294967296;
  };
}

export function generateDemoGraph(
  nodeCount = 500_000,
  edgeCount = 1_000_000,
  seed = 1337,
): GraphData {
  const rng = makeRng(seed);

  /* ---- cluster size distribution (power law) ---- */
  const clusterCount = 220;
  const raw = new Float64Array(clusterCount);
  let rawSum = 0;
  for (let c = 0; c < clusterCount; c++) {
    raw[c] = Math.pow(c + 1, -0.8);
    rawSum += raw[c];
  }
  const clusterSizes = new Uint32Array(clusterCount);
  let assigned = 0;
  for (let c = 0; c < clusterCount; c++) {
    const size = Math.max(24, Math.floor((raw[c] / rawSum) * nodeCount));
    clusterSizes[c] = size;
    assigned += size;
  }
  /* trim / pad the largest cluster so totals match exactly */
  clusterSizes[0] += nodeCount - assigned;

  const clusterStart = new Uint32Array(clusterCount + 1);
  for (let c = 0; c < clusterCount; c++) {
    clusterStart[c + 1] = clusterStart[c] + clusterSizes[c];
  }

  /* ---- cluster centers on a global phyllotaxis spiral ---- */
  const nodeSpacing = 1.6;
  const maxClusterRadius = nodeSpacing * Math.sqrt(clusterSizes[0]);
  const centerSpacing = maxClusterRadius * 1.05;
  const centersX = new Float64Array(clusterCount);
  const centersY = new Float64Array(clusterCount);
  /* biggest clusters in the middle, shuffled angle */
  for (let c = 0; c < clusterCount; c++) {
    const r = centerSpacing * Math.sqrt(c + 0.6);
    const a = c * GOLDEN_ANGLE;
    centersX[c] = r * Math.cos(a);
    centersY[c] = r * Math.sin(a);
  }

  /* ---- nodes ---- */
  const positions = new Float32Array(nodeCount * 2);
  const sizes = new Float32Array(nodeCount);
  const classes = new Uint8Array(nodeCount);
  const clusters = new Uint32Array(nodeCount);
  const bounds: Bounds = { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity };

  for (let c = 0; c < clusterCount; c++) {
    const start = clusterStart[c];
    const n = clusterSizes[c];
    const cx = centersX[c];
    const cy = centersY[c];
    const phase = rng() * Math.PI * 2;
    for (let k = 0; k < n; k++) {
      const i = start + k;
      const rr = nodeSpacing * Math.sqrt(k + 0.5);
      const aa = k * GOLDEN_ANGLE + phase;
      const jx = (rng() - 0.5) * nodeSpacing * 0.9;
      const jy = (rng() - 0.5) * nodeSpacing * 0.9;
      const x = cx + rr * Math.cos(aa) + jx;
      const y = cy + rr * Math.sin(aa) + jy;
      positions[i * 2] = x;
      positions[i * 2 + 1] = y;
      if (x < bounds.minX) bounds.minX = x;
      if (y < bounds.minY) bounds.minY = y;
      if (x > bounds.maxX) bounds.maxX = x;
      if (y > bounds.maxY) bounds.maxY = y;

      /* hubs: the first few nodes of each cluster are larger */
      const u = rng();
      sizes[i] =
        k === 0
          ? 14
          : k < 6
            ? 7 + u * 3
            : 1.6 + 4.5 * u * u * u;

      const cu = rng();
      let cls = 0;
      while (cls < CLASS_CDF.length - 1 && cu > CLASS_CDF[cls]) cls++;
      classes[i] = cls;
      clusters[i] = c;
    }
  }

  /* ---- edges: 85% intra-cluster (hub-biased), 15% inter-cluster ---- */
  const edges = new Uint32Array(edgeCount * 2);
  /* sample clusters proportionally to size via the node index space */
  let w = 0;
  for (let e = 0; e < edgeCount; e++) {
    if (rng() < 0.85) {
      /* intra: pick a random node, connect within its cluster, hub-biased */
      const i = (rng() * nodeCount) | 0;
      const c = clusters[i];
      const start = clusterStart[c];
      const n = clusterSizes[c];
      const u = rng();
      let j = start + ((u * u * n) | 0); /* quadratic bias toward hub end */
      if (j === i) j = start + (((j - start) + 1) % n);
      edges[w++] = i;
      edges[w++] = j;
    } else {
      /* inter: hub-biased endpoints from two clusters */
      const ca = (rng() * clusterCount) | 0;
      const cb = (rng() * clusterCount) | 0;
      const ua = rng();
      const ub = rng();
      edges[w++] = clusterStart[ca] + ((ua * ua * ua * clusterSizes[ca]) | 0);
      edges[w++] = clusterStart[cb] + ((ub * ub * ub * clusterSizes[cb]) | 0);
    }
  }

  const degrees = computeDegrees(edges, nodeCount);
  const clusterMeta: ClusterMeta[] = computeClusterMeta(
    positions,
    clusters,
    degrees,
    nodeCount,
  );
  for (let c = 0; c < clusterMeta.length; c++) {
    clusterMeta[c].name = clusterName(c);
  }

  /* ---- lazy string synthesis ---- */
  const clusterOf = (i: number) => clusters[i];
  const localIndex = (i: number) => i - clusterStart[clusterOf(i)];

  const name = (i: number) => {
    const a = WORDS_A[(i * 7 + clusterOf(i)) % WORDS_A.length];
    const b = WORDS_B[(i * 13) % WORDS_B.length];
    const cls = classes[i];
    switch (cls) {
      case 2: /* Class */
      case 3: /* Interface */
      case 5: /* Struct */
      case 7: /* Trait */
        return cap(a) + cap(b);
      case 4: /* File */
        return `${a}_${b}.rs`;
      default:
        return `${a}_${b}_${localIndex(i)}`;
    }
  };

  const file = (i: number) => {
    const m = MODULES[clusterOf(i) % MODULES.length];
    const b = WORDS_B[(i * 13) % WORDS_B.length];
    const line = 1 + ((i * 37) % 900);
    return `crates/aka-${m}/src/${b}.rs:${line}`;
  };

  return {
    count: nodeCount,
    edgeCount,
    positions,
    sizes,
    classes,
    clusters,
    edges,
    degrees,
    classNames: DEMO_CLASSES,
    clusterMeta,
    bounds,
    name,
    id: (i) => `n${i}`,
    file,
  };
}

function clusterName(c: number): string {
  const m = MODULES[c % MODULES.length];
  const b = WORDS_B[(c * 5 + 3) % WORDS_B.length];
  return `${m}/${b}`;
}

function cap(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}
