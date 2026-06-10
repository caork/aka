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

/** Mock repos — wired to Tauri commands in a later milestone. */
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
