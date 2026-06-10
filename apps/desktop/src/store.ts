import { create } from "zustand";

export type ViewId = "search" | "graph" | "symbol";

export type RepoStatus = "ready" | "indexing" | "idle";

export interface Repo {
  id: string;
  name: string;
  path: string;
  status: RepoStatus;
  symbols: number;
}

interface AppState {
  view: ViewId;
  setView(view: ViewId): void;

  repos: Repo[];
  selectedRepoId: string;
  selectRepo(id: string): void;

  query: string;
  setQuery(query: string): void;

  embeddingOn: boolean;
  toggleEmbedding(): void;
}

const SERVER = "http://127.0.0.1:4111";

/** 启动时从本地 `aka serve` 拉真实仓库列表；拉不到保留 mock 演示数据。 */
export async function hydrateRepos(): Promise<void> {
  try {
    const r = await fetch(`${SERVER}/api/repos`, {
      signal: AbortSignal.timeout(1500),
    });
    if (!r.ok) return;
    const body = (await r.json()) as {
      repos?: { name: string; path: string; nodes?: number }[];
    };
    const repos = body.repos ?? [];
    if (repos.length === 0) return;
    const mapped: Repo[] = repos.map((x) => ({
      id: x.name,
      name: x.name,
      path: x.path,
      status: "ready",
      symbols: x.nodes ?? 0,
    }));
    useAppStore.setState({ repos: mapped, selectedRepoId: mapped[0].id });
  } catch {
    /* server 未启动——保留演示数据 */
  }
}

/** Mock repos — `aka serve` 不在线时的演示数据。 */
const MOCK_REPOS: Repo[] = [
  {
    id: "aka",
    name: "aka",
    path: "~/workSpace/aka",
    status: "ready",
    symbols: 18432,
  },
  {
    id: "gitnexus",
    name: "GitNexus",
    path: "~/workSpace/GitNexus",
    status: "indexing",
    symbols: 52210,
  },
  {
    id: "tantivy",
    name: "tantivy",
    path: "~/oss/tantivy",
    status: "ready",
    symbols: 31876,
  },
  {
    id: "linux",
    name: "linux",
    path: "~/oss/linux",
    status: "idle",
    symbols: 0,
  },
];

export const useAppStore = create<AppState>((set) => ({
  view: "search",
  setView: (view) => set({ view }),

  repos: MOCK_REPOS,
  selectedRepoId: "aka",
  selectRepo: (selectedRepoId) => set({ selectedRepoId }),

  query: "",
  setQuery: (query) => set({ query }),

  embeddingOn: false,
  toggleEmbedding: () => set((s) => ({ embeddingOn: !s.embeddingOn })),
}));
