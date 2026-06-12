/* Real-data loader — pulls LOD / ego snapshots from the embedded desktop
   backend or a local `aka serve` instance (http://127.0.0.1:4111). */

import {
  RENDER_MAX_DEFAULT,
  RENDER_MAX_LIMIT,
  RENDER_MAX_MIN,
} from "../store";
import { invokeDesktop, isDesktopRuntime } from "../desktop-api";
import { apiUrl } from "../api-base";
import { parseGraphJSON, type GraphData, type GraphJSON } from "./format";

const EGO_MAX_NODES = 2_000;

/**
 * 指定 repo 的完整 LOD 快照；失败/离线返回 null。
 * `maxNodes` 来自 per-repo 的 render_max_nodes 设置（null/缺省 = 50_000），
 * 始终 clamp 到合同范围 [1_000, 500_000]。
 */
export async function loadRealGraph(
  repo: string,
  maxNodes?: number | null,
  signal?: AbortSignal,
): Promise<GraphData | null> {
  const budget = Math.min(
    RENDER_MAX_LIMIT,
    Math.max(RENDER_MAX_MIN, maxNodes ?? RENDER_MAX_DEFAULT),
  );
  try {
    const json = isDesktopRuntime()
      ? await invokeDesktop<GraphJSON>("graph_lod", { repo, maxNodes: budget })
      : await loadRealGraphHttp(repo, budget, signal);
    return parseGraphBody(json);
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    return null;
  }
}

async function loadRealGraphHttp(
  repo: string,
  budget: number,
  signal?: AbortSignal,
): Promise<GraphJSON> {
  const lr = await fetch(
    apiUrl(`/api/graph/lod?repo=${encodeURIComponent(repo)}&max_nodes=${budget}`),
    { signal: signal ?? AbortSignal.timeout(20_000) },
  );
  if (!lr.ok) throw new Error(String(lr.status));
  return (await lr.json()) as GraphJSON;
}

/** 以某节点为中心的 ego 子图（中心节点 i=0 在原点）；不支持/失败返回 null。 */
export async function loadEgoGraph(
  repo: string,
  id: string,
  depth = 2,
  signal?: AbortSignal,
): Promise<GraphData | null> {
  try {
    const json = isDesktopRuntime()
      ? await invokeDesktop<GraphJSON>("graph_ego", {
          repo,
          id,
          depth,
          maxNodes: EGO_MAX_NODES,
        })
      : await loadEgoGraphHttp(repo, id, depth, signal);
    return parseGraphBody(json);
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    return null;
  }
}

async function loadEgoGraphHttp(
  repo: string,
  id: string,
  depth: number,
  signal?: AbortSignal,
): Promise<GraphJSON> {
  const lr = await fetch(
    apiUrl(`/api/graph/ego?repo=${encodeURIComponent(repo)}&id=${encodeURIComponent(id)}&depth=${depth}&max_nodes=${EGO_MAX_NODES}`),
    { signal: signal ?? AbortSignal.timeout(20_000) },
  );
  if (!lr.ok) throw new Error(String(lr.status));
  return (await lr.json()) as GraphJSON;
}

function parseGraphBody(json: GraphJSON): GraphData | null {
  if (!json.nodes?.length) return null;
  /* renderer 的 edgeFraction 远景采样假设边序无偏；服务端按源节点
     排序输出，这里洗牌一次（确定性 LCG，刷新结果稳定）。 */
  shuffleEdgePairs(json.edges);
  return parseGraphJSON(json);
}

function shuffleEdgePairs(edges: number[]): void {
  let seed = 0x9e3779b9 >>> 0;
  const rand = () => {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    return seed / 0x1_0000_0000;
  };
  for (let i = edges.length / 2 - 1; i > 0; i--) {
    const j = Math.floor(rand() * (i + 1));
    const a = 2 * i;
    const b = 2 * j;
    [edges[a], edges[b]] = [edges[b], edges[a]];
    [edges[a + 1], edges[b + 1]] = [edges[b + 1], edges[a + 1]];
  }
}
