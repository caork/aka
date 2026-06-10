import { create } from "zustand";

export type ViewId = "search" | "graph" | "symbol";

export type RepoStatus = "ready" | "indexing" | "failed" | "idle";

export type RepoSourceKind = "local" | "git" | "zip";

export interface RepoSource {
  kind: RepoSourceKind;
  url: string | null;
}

export interface Repo {
  id: string;
  name: string;
  path: string;
  status: RepoStatus;
  symbols: number;
  embeddings: boolean;
  source: RepoSource;
  /** failed 时的错误信息 */
  detail: string | null;
}

interface AppState {
  view: ViewId;
  setView(view: ViewId): void;

  repos: Repo[];
  /** repos 是否来自真实 `aka serve`（false = mock 演示数据） */
  reposLive: boolean;
  selectedRepoId: string;
  selectRepo(id: string): void;

  query: string;
  setQuery(query: string): void;
}

const SERVER = "http://127.0.0.1:4111";
const SELECTED_KEY = "aka.selectedRepo";

interface RepoOut {
  name: string;
  path: string;
  nodes?: number;
  edges?: number;
  embeddings?: boolean;
  status?: string;
  source?: { kind?: string; url?: string | null } | null;
  detail?: string | null;
}

function mapRepo(x: RepoOut): Repo {
  const status: RepoStatus =
    x.status === "indexing" || x.status === "failed" ? x.status : "ready";
  const kind: RepoSourceKind =
    x.source?.kind === "git" || x.source?.kind === "zip"
      ? x.source.kind
      : "local";
  return {
    id: x.name,
    name: x.name,
    path: x.path,
    status,
    symbols: x.nodes ?? 0,
    embeddings: x.embeddings ?? false,
    source: { kind, url: x.source?.url ?? null },
    detail: x.detail ?? null,
  };
}

let pollTimer: number | null = null;
let refreshSeq = 0;

/**
 * 从本地 `aka serve` 拉取仓库列表，可重复调用（导入/更新后刷新）。
 * 只要存在 "indexing" 状态的仓库就每 3s 轮询，直到全部 ready/failed。
 * 服务不可达时保留当前数据（首次即 mock 演示数据）。
 */
export async function refreshRepos(): Promise<void> {
  const seq = ++refreshSeq;
  try {
    const r = await fetch(`${SERVER}/api/repos`, {
      signal: AbortSignal.timeout(2500),
    });
    if (!r.ok) return;
    const body = (await r.json()) as { repos?: RepoOut[] };
    if (seq !== refreshSeq) return; /* 已被更新的一次刷新取代 */
    const repos = (body.repos ?? []).map(mapRepo);
    if (repos.length === 0) return;

    const current = useAppStore.getState().selectedRepoId;
    const persisted = readPersistedSelection();
    const selectedRepoId = repos.some((x) => x.id === current)
      ? current
      : persisted && repos.some((x) => x.id === persisted)
        ? persisted
        : repos[0].id;

    useAppStore.setState({ repos, selectedRepoId, reposLive: true });
    schedulePoll(repos.some((x) => x.status === "indexing"));
  } catch {
    /* server 未启动——保留现有数据 */
  }
}

function schedulePoll(needed: boolean): void {
  stopRepoPolling();
  if (!needed) return;
  pollTimer = window.setTimeout(() => {
    pollTimer = null;
    void refreshRepos();
  }, 3000);
}

/** 清理 indexing 轮询定时器。 */
export function stopRepoPolling(): void {
  if (pollTimer !== null) {
    window.clearTimeout(pollTimer);
    pollTimer = null;
  }
}

function readPersistedSelection(): string | null {
  try {
    return window.localStorage.getItem(SELECTED_KEY);
  } catch {
    return null;
  }
}

function persistSelection(id: string): void {
  try {
    window.localStorage.setItem(SELECTED_KEY, id);
  } catch {
    /* private mode 等场景忽略 */
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
    embeddings: false,
    source: { kind: "local", url: null },
    detail: null,
  },
  {
    id: "gitnexus",
    name: "GitNexus",
    path: "~/workSpace/GitNexus",
    status: "indexing",
    symbols: 52210,
    embeddings: false,
    source: { kind: "git", url: "https://github.com/caork/GitNexus" },
    detail: null,
  },
  {
    id: "tantivy",
    name: "tantivy",
    path: "~/oss/tantivy",
    status: "ready",
    symbols: 31876,
    embeddings: true,
    source: { kind: "local", url: null },
    detail: null,
  },
  {
    id: "linux",
    name: "linux",
    path: "~/oss/linux",
    status: "idle",
    symbols: 0,
    embeddings: false,
    source: { kind: "local", url: null },
    detail: null,
  },
];

export const useAppStore = create<AppState>((set) => ({
  view: "search",
  setView: (view) => set({ view }),

  repos: MOCK_REPOS,
  reposLive: false,
  selectedRepoId: readPersistedSelection() ?? "aka",
  selectRepo: (selectedRepoId) => {
    persistSelection(selectedRepoId);
    set({ selectedRepoId });
  },

  query: "",
  setQuery: (query) => set({ query }),
}));
