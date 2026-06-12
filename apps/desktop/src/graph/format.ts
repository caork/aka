/**
 * Graph wire format — contract with the Rust side (aka-graph LOD snapshots).
 *
 * JSON shape:
 * {
 *   "classes": ["Function", "Class", ...],
 *   "nodes":   [{ "i": u32, "id": str, "x": f32, "y": f32, "s": f32,
 *                 "c": clusterIdx, "l": classIdx, "name": str }, ...],
 *   "edges":   [s0, t0, s1, t1, ...]  // flat u32 pairs into nodes[].i
 * }
 *
 * Before rendering everything is converted to TypedArrays; per-node strings
 * (id / name) stay behind accessor functions so synthetic datasets can
 * generate them lazily instead of materialising 500k strings.
 */

export interface GraphJSONNode {
  i: number;
  id: string;
  x: number;
  y: number;
  s: number;
  c: number;
  l: number;
  name: string;
}

export interface GraphJSON {
  classes: string[];
  nodes: GraphJSONNode[];
  edges: number[];
  edge_weights?: number[];
  cluster_labels?: string[];
}

export interface Bounds {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

export interface ClusterMeta {
  /** label shown at mid-zoom LOD */
  name: string;
  /** centroid in world space */
  x: number;
  y: number;
  /** total degree of member nodes — label priority */
  weight: number;
}

export interface GraphData {
  count: number;
  edgeCount: number;
  /** xy interleaved, length 2*count */
  positions: Float32Array;
  /** world-space diameter, length count */
  sizes: Float32Array;
  /** index into classNames, length count */
  classes: Uint8Array;
  /** cluster id per node, length count */
  clusters: Uint32Array;
  /** flat (source, target) pairs, length 2*edgeCount */
  edges: Uint32Array;
  /** degree per node — label/picking priority */
  degrees: Uint32Array;
  classNames: string[];
  clusterMeta: ClusterMeta[];
  bounds: Bounds;
  name(i: number): string;
  id(i: number): string;
  file(i: number): string;
}

/** Convert the JSON wire format into render-ready TypedArrays. */
export function parseGraphJSON(json: GraphJSON): GraphData {
  const n = json.nodes.length;
  const positions = new Float32Array(n * 2);
  const sizes = new Float32Array(n);
  const classes = new Uint8Array(n);
  const clusters = new Uint32Array(n);
  const names = new Array<string>(n);
  const ids = new Array<string>(n);

  const bounds: Bounds = {
    minX: Infinity,
    minY: Infinity,
    maxX: -Infinity,
    maxY: -Infinity,
  };

  for (const node of json.nodes) {
    const i = node.i;
    positions[i * 2] = node.x;
    positions[i * 2 + 1] = node.y;
    sizes[i] = node.s;
    classes[i] = node.l;
    clusters[i] = node.c;
    names[i] = node.name;
    ids[i] = node.id;
    if (node.x < bounds.minX) bounds.minX = node.x;
    if (node.y < bounds.minY) bounds.minY = node.y;
    if (node.x > bounds.maxX) bounds.maxX = node.x;
    if (node.y > bounds.maxY) bounds.maxY = node.y;
  }

  const edges = Uint32Array.from(json.edges);
  const edgeCount = edges.length >> 1;
  const degrees = computeDegrees(edges, n, json.edge_weights);
  const clusterMeta = computeClusterMeta(
    positions,
    clusters,
    degrees,
    n,
    json.cluster_labels,
  );

  return {
    count: n,
    edgeCount,
    positions,
    sizes,
    classes,
    clusters,
    edges,
    degrees,
    classNames: json.classes,
    clusterMeta,
    bounds,
    name: (i) => names[i] ?? "",
    id: (i) => ids[i] ?? "",
    file: () => "",
  };
}

export function computeDegrees(
  edges: Uint32Array,
  nodeCount: number,
  edgeWeights?: number[],
): Uint32Array {
  const degrees = new Uint32Array(nodeCount);
  for (let e = 0; e < edges.length; e++) {
    const weight = edgeWeights?.[e >> 1] ?? 1;
    degrees[edges[e]] += Math.max(1, Math.round(weight));
  }
  return degrees;
}

export function computeClusterMeta(
  positions: Float32Array,
  clusters: Uint32Array,
  degrees: Uint32Array,
  count: number,
  clusterLabels?: string[],
): ClusterMeta[] {
  let maxCluster = 0;
  for (let i = 0; i < count; i++) {
    if (clusters[i] > maxCluster) maxCluster = clusters[i];
  }
  const k = maxCluster + 1;
  const sumX = new Float64Array(k);
  const sumY = new Float64Array(k);
  const num = new Uint32Array(k);
  const weight = new Float64Array(k);
  for (let i = 0; i < count; i++) {
    const c = clusters[i];
    sumX[c] += positions[i * 2];
    sumY[c] += positions[i * 2 + 1];
    num[c]++;
    weight[c] += degrees[i];
  }
  const meta: ClusterMeta[] = [];
  for (let c = 0; c < k; c++) {
    if (num[c] === 0) continue;
    meta.push({
      name: clusterLabels?.[c] ?? `cluster ${c}`,
      x: sumX[c] / num[c],
      y: sumY[c] / num[c],
      weight: weight[c],
    });
  }
  return meta;
}
