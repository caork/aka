/* Real-data loader — pulls the LOD snapshot from a local `aka serve`
   instance (http://127.0.0.1:4111). Returns null when the server is not
   running / has no indexed repos, so the caller can fall back to the
   synthetic demo graph. */

import { parseGraphJSON, type GraphData, type GraphJSON } from "./format";

const SERVER = "http://127.0.0.1:4111";
const MAX_NODES = 300_000;

interface RepoOut {
  name: string;
  nodes?: number;
}

export interface RealGraph {
  data: GraphData;
  repo: string;
}

export async function loadRealGraph(): Promise<RealGraph | null> {
  try {
    const rr = await fetch(`${SERVER}/api/repos`, {
      signal: AbortSignal.timeout(1500),
    });
    if (!rr.ok) return null;
    const body = (await rr.json()) as { repos?: RepoOut[] };
    const repos = body.repos ?? [];
    if (repos.length === 0) return null;

    /* 节点最多的仓库最值得看 */
    const best = [...repos].sort((a, b) => (b.nodes ?? 0) - (a.nodes ?? 0))[0];
    const lr = await fetch(
      `${SERVER}/api/graph/lod?repo=${encodeURIComponent(best.name)}&max_nodes=${MAX_NODES}`,
      { signal: AbortSignal.timeout(20_000) },
    );
    if (!lr.ok) return null;
    const json = (await lr.json()) as GraphJSON;
    if (!json.nodes?.length) return null;

    /* renderer 的 edgeFraction 远景采样假设边序无偏；服务端按源节点
       排序输出，这里洗牌一次（确定性 LCG，刷新结果稳定）。 */
    shuffleEdgePairs(json.edges);
    return { data: parseGraphJSON(json), repo: best.name };
  } catch {
    return null;
  }
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
