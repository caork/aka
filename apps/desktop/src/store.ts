import { create } from "zustand";

export type ViewId = "search" | "graph" | "symbol" | "code";

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
  /** 图渲染节点上限（null = 默认 50_000） */
  renderMaxNodes: number | null;
  source: RepoSource;
  /** failed 时的错误信息 */
  detail: string | null;
}

/* 渲染预算（与后端合同一致）：默认 5 万，硬上限 50 万，最小 1 千。 */
export const RENDER_MAX_DEFAULT = 50_000;
export const RENDER_MAX_LIMIT = 500_000;
export const RENDER_MAX_MIN = 1_000;

/** 详情侧边栏目标 —— Search/Graph/Symbol 三视图共用。 */
export interface DetailTarget {
  id: string;
  name: string;
  label: string;
  file: string;
  line: number;
}

/** DetailPanel「Ego 视图」→ GraphView 的下钻请求。 */
export interface EgoRequest {
  id: string;
  name: string;
}

/** Code 视图（GitHub 式全文源码预览）的打开目标。 */
export interface CodeTarget {
  repo: string;
  /** 仓库内相对路径 */
  path: string;
  /** 打开后滚动定位到的行（1-based），可选 */
  line?: number;
  /** 高亮范围结束行，可选 */
  endLine?: number;
}

/** CodeView 符号点击 → GraphView「定位到该节点」请求。 */
export interface FocusRequest {
  id: string;
  name: string;
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

  /** 右侧详情面板（null = 关闭） */
  detailTarget: DetailTarget | null;
  openDetail(target: DetailTarget): void;
  closeDetail(): void;

  /** 待 GraphView 消费的 ego 下钻请求 */
  egoRequest: EgoRequest | null;
  /** 切到 Graph 视图并请求以该节点为中心下钻（同时关闭详情面板） */
  requestEgo(id: string, name: string): void;
  clearEgoRequest(): void;

  /** Code 视图目标（null = 无打开的文件；view === "code" 时必有值） */
  codeTarget: CodeTarget | null;
  /** 打开 Code 视图定位到文件/行；记住来源视图供 closeCode 返回 */
  openCode(target: CodeTarget): void;
  /** 关闭 Code 视图，返回打开前的视图 */
  closeCode(): void;

  /** 待 GraphView 消费的「定位到节点」请求（切到 graph 并居中选中该节点） */
  focusRequest: FocusRequest | null;
  requestFocus(id: string, name: string): void;
  clearFocusRequest(): void;
}

const SERVER = "http://127.0.0.1:4111";
const SELECTED_KEY = "aka.selectedRepo";

interface RepoOut {
  name: string;
  path: string;
  nodes?: number;
  edges?: number;
  embeddings?: boolean;
  render_max_nodes?: number | null;
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
    renderMaxNodes:
      typeof x.render_max_nodes === "number" ? x.render_max_nodes : null,
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
    renderMaxNodes: null,
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
    renderMaxNodes: null,
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
    renderMaxNodes: null,
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
    renderMaxNodes: null,
    source: { kind: "local", url: null },
    detail: null,
  },
];

/** openCode 之前所在的视图，closeCode 用于返回。 */
let codeReturnView: ViewId = "search";

export const useAppStore = create<AppState>((set, get) => ({
  view: "search",
  setView: (view) => set({ view }),

  repos: MOCK_REPOS,
  reposLive: false,
  selectedRepoId: readPersistedSelection() ?? "aka",
  selectRepo: (selectedRepoId) => {
    persistSelection(selectedRepoId);
    /* 换仓库时关闭详情面板与 Code 视图，避免跨仓库的陈旧目标 */
    set((s) => ({
      selectedRepoId,
      detailTarget: null,
      codeTarget: null,
      view: s.view === "code" ? "search" : s.view,
    }));
  },

  query: "",
  setQuery: (query) => set({ query }),

  detailTarget: null,
  openDetail: (detailTarget) => set({ detailTarget }),
  closeDetail: () => set({ detailTarget: null }),

  egoRequest: null,
  requestEgo: (id, name) =>
    set({ egoRequest: { id, name }, view: "graph", detailTarget: null }),
  clearEgoRequest: () => set({ egoRequest: null }),

  codeTarget: null,
  openCode: (codeTarget) => {
    const current = get().view;
    if (current !== "code") codeReturnView = current;
    set({ codeTarget, view: "code", detailTarget: null });
  },
  closeCode: () => set({ codeTarget: null, view: codeReturnView }),

  focusRequest: null,
  requestFocus: (id, name) =>
    set({ focusRequest: { id, name }, view: "graph" }),
  clearFocusRequest: () => set({ focusRequest: null }),
}));
