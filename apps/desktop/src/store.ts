import { create } from "zustand";
import {
  applyTheme,
  persistThemeMode,
  readThemeMode,
  resolveThemeMode,
  type ResolvedTheme,
  type ThemeMode,
} from "./theme";
import { invokeDesktop, isDesktopRuntime } from "./desktop-api";
import { apiUrl } from "./api-base";

export type ViewId = "code" | "graph";

export type RepoStatus = "ready" | "indexing" | "failed" | "idle";

export type RepoSourceKind = "local" | "git" | "zip";

export interface RepoSource {
  kind: RepoSourceKind;
  url: string | null;
}

export interface RepoProgress {
  stage: string;
  message: string;
  percent: number;
  current: number | null;
  total: number | null;
  files: number;
  nodes: number;
  edges: number;
  chunks: number;
  logs: string[];
}

export interface Repo {
  id: string;
  name: string;
  description: string | null;
  path: string;
  status: RepoStatus;
  symbols: number;
  embeddings: boolean;
  /** 图渲染节点上限（null = 默认 50_000） */
  renderMaxNodes: number | null;
  source: RepoSource;
  /** failed 时的错误信息 */
  detail: string | null;
  /** indexing / failed 时的实时进度和日志 */
  progress: RepoProgress | null;
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
  themeMode: ThemeMode;
  resolvedTheme: ResolvedTheme;
  setThemeMode(themeMode: ThemeMode): void;
  syncSystemTheme(): void;

  view: ViewId;
  setView(view: ViewId): void;

  repos: Repo[];
  selectedRepoId: string;
  selectRepo(id: string): void;
  resetRepos(): void;

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
  /** 打开 Code 视图定位到文件/行；可同步右侧详情锚点 */
  openCode(target: CodeTarget): void;
  /** 关闭 Code 视图，返回打开前的视图 */
  closeCode(): void;

  /** 待 GraphView 消费的「定位到节点」请求（切到 graph 并居中选中该节点） */
  focusRequest: FocusRequest | null;
  requestFocus(id: string, name: string): void;
  clearFocusRequest(): void;
}

const SELECTED_KEY = "aka.selectedRepo";
const INITIAL_THEME_MODE = readThemeMode();
const INITIAL_RESOLVED_THEME = resolveThemeMode(INITIAL_THEME_MODE);

interface RepoOut {
  name: string;
  description?: string | null;
  path: string;
  nodes?: number;
  edges?: number;
  embeddings?: boolean;
  render_max_nodes?: number | null;
  status?: string;
  source?: { kind?: string; url?: string | null } | null;
  detail?: string | null;
  progress?: Partial<RepoProgress> | null;
}

function mapRepo(x: RepoOut): Repo {
  const status: RepoStatus =
    x.status === "indexing" || x.status === "failed" ? x.status : "ready";
  const kind: RepoSourceKind =
    x.source?.kind === "git" || x.source?.kind === "zip"
      ? x.source.kind
      : "local";
  const progressPercent =
    status === "ready" && x.progress ? 100 : clampPercent(x.progress?.percent);
  return {
    id: x.name,
    name: x.name,
    description:
      typeof x.description === "string" && x.description.trim().length > 0
        ? x.description
        : null,
    path: x.path,
    status,
    symbols: x.nodes ?? 0,
    embeddings: x.embeddings ?? false,
    renderMaxNodes:
      typeof x.render_max_nodes === "number" ? x.render_max_nodes : null,
    source: { kind, url: x.source?.url ?? null },
    detail: x.detail ?? null,
    progress: x.progress
      ? {
          stage: x.progress.stage ?? status,
          message: x.progress.message ?? status,
          percent: progressPercent,
          current: typeof x.progress.current === "number" ? x.progress.current : null,
          total: typeof x.progress.total === "number" ? x.progress.total : null,
          files: x.progress.files ?? 0,
          nodes: x.progress.nodes ?? 0,
          edges: x.progress.edges ?? 0,
          chunks: x.progress.chunks ?? 0,
          logs: Array.isArray(x.progress.logs) ? x.progress.logs : [],
        }
      : null,
  };
}

function clampPercent(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.max(0, Math.min(100, value))
    : 0;
}

let pollTimer: number | null = null;
let refreshSeq = 0;

/**
 * 从本地 `aka serve` 拉取仓库列表，可重复调用（导入/更新后刷新）。
 * 只要存在 "indexing" 状态的仓库就每 3s 轮询，直到全部 ready/failed。
 * 服务不可达时保留当前状态；桌面端首次启动为空仓库列表。
 */
export async function refreshRepos(): Promise<void> {
  const seq = ++refreshSeq;
  try {
    const desktop = isDesktopRuntime();
    const body = isDesktopRuntime()
      ? await invokeDesktop<{ repos?: RepoOut[] }>("list_repos")
      : await fetchReposHttp();
    if (seq !== refreshSeq) return; /* 已被更新的一次刷新取代 */
    const repos = (body.repos ?? []).map(mapRepo);
    if (repos.length === 0) {
      if (desktop) {
        useAppStore.setState({
          repos: [],
          selectedRepoId: "",
          detailTarget: null,
          codeTarget: null,
          egoRequest: null,
          focusRequest: null,
          query: "",
        });
        schedulePoll(false);
      }
      return;
    }

    const current = useAppStore.getState().selectedRepoId;
    const persisted = readPersistedSelection();
    const selectedRepoId = repos.some((x) => x.id === current)
      ? current
      : persisted && repos.some((x) => x.id === persisted)
        ? persisted
        : repos[0].id;

    useAppStore.setState({ repos, selectedRepoId });
    schedulePoll(repos.some((x) => x.status === "indexing"));
  } catch {
    /* server 未启动——保留现有数据 */
  }
}

async function fetchReposHttp(): Promise<{ repos?: RepoOut[] }> {
  const r = await fetch(apiUrl("/api/repos"), {
    signal: AbortSignal.timeout(2500),
  });
  if (!r.ok) return {};
  return (await r.json()) as { repos?: RepoOut[] };
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

function clearPersistedSelection(): void {
  try {
    window.localStorage.removeItem(SELECTED_KEY);
  } catch {
    /* private mode 等场景忽略 */
  }
}

export const useAppStore = create<AppState>((set) => ({
  themeMode: INITIAL_THEME_MODE,
  resolvedTheme: INITIAL_RESOLVED_THEME,
  setThemeMode: (themeMode) => {
    persistThemeMode(themeMode);
    const resolvedTheme = resolveThemeMode(themeMode);
    applyTheme(themeMode, resolvedTheme);
    set({ themeMode, resolvedTheme });
  },
  syncSystemTheme: () => {
    const themeMode = useAppStore.getState().themeMode;
    const resolvedTheme = resolveThemeMode(themeMode);
    applyTheme(themeMode, resolvedTheme);
    set({ resolvedTheme });
  },

  view: "code",
  setView: (view) => set({ view }),

  repos: [],
  selectedRepoId: readPersistedSelection() ?? "",
  selectRepo: (selectedRepoId) => {
    persistSelection(selectedRepoId);
    set({ selectedRepoId, detailTarget: null, codeTarget: null });
  },
  resetRepos: () => {
    clearPersistedSelection();
    stopRepoPolling();
    set({
      repos: [],
      selectedRepoId: "",
      detailTarget: null,
      egoRequest: null,
      codeTarget: null,
      focusRequest: null,
      query: "",
    });
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
    const state = useAppStore.getState();
    const detailTarget = state.detailTarget;
    const nextDetail =
      detailTarget && detailTarget.file === codeTarget.path
        ? {
            ...detailTarget,
            line: codeTarget.line ?? detailTarget.line,
          }
        : detailTarget;
    set({ codeTarget, view: "code", detailTarget: nextDetail });
  },
  closeCode: () => set({ codeTarget: null }),

  focusRequest: null,
  requestFocus: (id, name) =>
    set({ focusRequest: { id, name }, view: "graph" }),
  clearFocusRequest: () => set({ focusRequest: null }),
}));
