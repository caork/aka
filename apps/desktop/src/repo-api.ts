/* Repo 管理 API —— 与后端 agent 锁定的合同（见仓库 README / 任务说明）。
   旧后端尚未实现这些端点时返回 404/501，所有调用方都必须优雅降级。 */

const SERVER = "http://127.0.0.1:4111";

export class ApiError extends Error {
  status: number;
  /** true = 后端版本太旧，端点不存在（404/501） */
  unsupported: boolean;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
    this.unsupported = status === 404 || status === 501;
  }
}

async function ensureOk(r: Response): Promise<void> {
  if (r.ok) return;
  let message = `请求失败（HTTP ${r.status}）`;
  if (r.status === 404 || r.status === 501) {
    message = "当前后端不支持该操作，需更新 aka serve";
  } else {
    try {
      const text = (await r.text()).trim();
      if (text) message = text.slice(0, 300);
    } catch {
      /* body 不可读则保留默认信息 */
    }
  }
  throw new ApiError(r.status, message);
}

function asError(e: unknown): Error {
  if (e instanceof ApiError) return e;
  if (e instanceof DOMException && e.name === "TimeoutError") {
    return new Error("请求超时——aka serve 是否在运行？");
  }
  return new Error("无法连接本地 aka serve（127.0.0.1:4111）");
}

async function postJson(path: string, body: unknown, timeout = 8000): Promise<void> {
  let r: Response;
  try {
    r = await fetch(`${SERVER}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(timeout),
    });
  } catch (e) {
    throw asError(e);
  }
  await ensureOk(r);
}

async function postForm(path: string, form: FormData, timeout = 60_000): Promise<void> {
  let r: Response;
  try {
    r = await fetch(`${SERVER}${path}`, {
      method: "POST",
      body: form,
      signal: AbortSignal.timeout(timeout),
    });
  } catch (e) {
    throw asError(e);
  }
  await ensureOk(r);
}

export type ImportInput =
  | { kind: "git"; url: string; name?: string }
  | { kind: "local"; path: string };

export async function importRepo(input: ImportInput): Promise<void> {
  await postJson("/api/repos/import", input);
}

export async function importZip(name: string, file: File): Promise<void> {
  const form = new FormData();
  form.append("name", name);
  form.append("file", file);
  await postForm("/api/repos/import-zip", form);
}

export async function updateRepo(name: string): Promise<void> {
  await postJson(`/api/repos/${encodeURIComponent(name)}/update`, {});
}

export async function updateZip(name: string, file: File): Promise<void> {
  const form = new FormData();
  form.append("file", file);
  await postForm(`/api/repos/${encodeURIComponent(name)}/update-zip`, form);
}

export async function setRepoSettings(
  name: string,
  embeddingsEnabled: boolean,
): Promise<void> {
  await postJson(`/api/repos/${encodeURIComponent(name)}/settings`, {
    embeddings_enabled: embeddingsEnabled,
  });
}

export async function deleteRepo(name: string): Promise<void> {
  let r: Response;
  try {
    r = await fetch(`${SERVER}/api/repos/${encodeURIComponent(name)}`, {
      method: "DELETE",
      signal: AbortSignal.timeout(8000),
    });
  } catch (e) {
    throw asError(e);
  }
  await ensureOk(r);
}

/* ---- 节点详情 ---- */

export interface NodeDetail {
  id: string;
  name: string;
  label: string;
  file: string;
  line: number;
  end_line: number;
  properties: Record<string, unknown>;
  degree: { callers: number; callees: number; refs: number };
}

export type NodeDetailResult =
  | { state: "ok"; detail: NodeDetail }
  | { state: "unsupported" }
  | { state: "offline" };

export async function fetchNodeDetail(
  repo: string,
  id: string,
  signal?: AbortSignal,
): Promise<NodeDetailResult> {
  try {
    const r = await fetch(
      `${SERVER}/api/node?repo=${encodeURIComponent(repo)}&id=${encodeURIComponent(id)}`,
      { signal: signal ?? AbortSignal.timeout(4000) },
    );
    if (r.status === 404 || r.status === 501) return { state: "unsupported" };
    if (!r.ok) return { state: "offline" };
    const detail = (await r.json()) as NodeDetail;
    return { state: "ok", detail };
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    return { state: "offline" };
  }
}
