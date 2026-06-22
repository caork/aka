import type { Repo } from "./store";

export function indexLogLines(repo: Repo): string[] {
  if (repo.progress?.logs && repo.progress.logs.length > 0) return repo.progress.logs;
  if (repo.detail) return [repo.detail];
  return [repo.status === "failed" ? "Indexing failed" : "Waiting for indexing events"];
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
  if (!progress) return "indexing...";
  const percent = `${Math.round(progress.percent)}%`;
  const label = progress.message || progress.stage || "indexing";
  return `${percent} · ${label}`;
}
