import type { Repo } from "./store";

export const INDEX_PHASES = [
  { key: "queued", label: "Queued" },
  { key: "engine", label: "Parse" },
  { key: "adapter", label: "Artifacts" },
  { key: "index", label: "Index" },
  { key: "register", label: "Register" },
] as const;

export function indexLogLines(repo: Repo): string[] {
  if (repo.progress?.logs && repo.progress.logs.length > 0) return repo.progress.logs;
  if (repo.detail) return [repo.detail];
  if (repo.status === "ready") {
    return ["Index ready. No live indexing events are retained for this session."];
  }
  if (repo.status === "failed") return ["Indexing failed"];
  return ["Indexing is active. Waiting for the next backend event."];
}

export function buildIndexLogText(repo: Repo): string {
  const progress = repo.progress;
  return [
    `repo=${repo.name}`,
    `path=${repo.path}`,
    `status=${repo.status}`,
    `source=${repo.source.kind}${repo.source.url ? ` ${repo.source.url}` : ""}`,
    progress ? `stage=${progress.stage}` : "",
    progress ? `message=${progress.message}` : "",
    progress ? `percent=${Math.round(progress.percent * 10) / 10}` : "",
    progress && progress.current !== null ? `current=${progress.current}` : "",
    progress && progress.total !== null ? `total=${progress.total}` : "",
    progress ? `files=${progress.files}` : "",
    progress ? `nodes=${progress.nodes}` : "",
    progress ? `edges=${progress.edges}` : "",
    progress ? `chunks=${progress.chunks}` : "",
    repo.detail ? `detail=${repo.detail}` : "",
    "",
    ...indexLogLines(repo),
  ]
    .filter(Boolean)
    .join("\n");
}

export function compactIndexStatus(repo: Repo): string {
  const progress = repo.progress;
  if (!progress) return "indexing";
  const phase = indexPhaseLabel(progress.stage);
  const count = formatProgressCount(progress);
  const label = progress.message || progress.stage || "indexing";
  return count ? `${phase} · ${count}` : `${phase} · ${label}`;
}

export function indexPhaseLabel(stage: string | null | undefined): string {
  const normalized = (stage ?? "").toLowerCase();
  if (normalized === "done") return "Ready";
  if (normalized === "failed") return "Failed";
  if (normalized.includes("register")) return "Register";
  if (
    normalized.startsWith("graph") ||
    normalized.startsWith("search") ||
    normalized.startsWith("incremental") ||
    normalized.includes("index")
  ) {
    return "Index";
  }
  if (normalized.includes("adapter") || normalized.includes("artifact")) return "Artifacts";
  if (normalized.includes("queued")) return "Queued";
  return "Parse";
}

export function activeIndexPhase(repo: Repo): number {
  if (repo.status === "ready") return INDEX_PHASES.length;
  const stage = repo.progress?.stage ?? "";
  const label = indexPhaseLabel(stage);
  const idx = INDEX_PHASES.findIndex((phase) => phase.label === label);
  return idx >= 0 ? idx : 0;
}

export function formatProgressCount(progress: Repo["progress"]): string {
  if (!progress) return "";
  if (progress.current !== null && progress.total !== null && progress.total > 0) {
    return `${progress.current.toLocaleString()} / ${progress.total.toLocaleString()}`;
  }
  if (progress.current !== null && progress.current > 0) {
    return progress.current.toLocaleString();
  }
  return "";
}
