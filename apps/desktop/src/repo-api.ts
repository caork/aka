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

export interface RepoSettingsInput {
  embeddingsEnabled: boolean;
  /** 图渲染节点上限；null = 使用默认（50_000） */
  renderMaxNodes: number | null;
}

export async function setRepoSettings(
  name: string,
  settings: RepoSettingsInput,
): Promise<void> {
  await postJson(`/api/repos/${encodeURIComponent(name)}/settings`, {
    embeddings_enabled: settings.embeddingsEnabled,
    render_max_nodes: settings.renderMaxNodes,
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

/* ---- 流程（Process 合成节点）语义 ----
   engine 检测出的调用链节点（label=="Process"）没有自身源码位置，
   后端在 /api/node 响应里附带流程语义；旧后端无这些字段，调用方必须防御。 */

/** 流程内的符号引用（入口 entry / 终点 terminal 共用形状） */
export interface ProcessSymbolRef {
  id: string;
  name: string;
  label: string;
  file: string;
  line: number;
}

/** 流程步骤（steps 按 step 升序） */
export interface ProcessStep extends ProcessSymbolRef {
  /** 1-based 步序 */
  step: number;
}

export interface ProcessInfo {
  /** "cross_community" | "intra_community"（前向兼容保留 string） */
  process_type: string;
  step_count: number;
  entry: ProcessSymbolRef | null;
  terminal: ProcessSymbolRef | null;
  steps: ProcessStep[];
}

/** 普通符号节点参与的某条流程 */
export interface ProcessMembership {
  /** 对应 Process 节点的 id，可直接用于 /api/node 查询 */
  process_id: string;
  name: string;
  process_type: string;
  /** 该符号在流程中的步序（1-based） */
  step: number;
  step_count: number;
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
  /** 仅 label=="Process" 的合成流程节点附带（旧后端无此字段） */
  process?: ProcessInfo | null;
  /** 该符号参与的流程列表，可能为空数组（旧后端无此字段） */
  processes?: ProcessMembership[] | null;
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

/* ---- 源码片段（GET /api/source，合同：start/end 1-based 含端） ---- */

export interface SourceSlice {
  path: string;
  abs_path: string;
  total_lines: number;
  start: number;
  end: number;
  lines: string[];
  truncated: boolean;
}

export type SourceResult =
  | { state: "ok"; source: SourceSlice }
  /** 404/501 —— 后端版本不支持该端点 */
  | { state: "unsupported" }
  /** 400 —— 非文本文件 */
  | { state: "binary" }
  | { state: "offline" };

export async function fetchSource(
  repo: string,
  path: string,
  start?: number,
  end?: number,
  signal?: AbortSignal,
): Promise<SourceResult> {
  const params = new URLSearchParams({ repo, path });
  if (start !== undefined) params.set("start", String(start));
  if (end !== undefined) params.set("end", String(end));
  try {
    const r = await fetch(`${SERVER}/api/source?${params.toString()}`, {
      signal: signal ?? AbortSignal.timeout(6000),
    });
    if (r.status === 404 || r.status === 501) return { state: "unsupported" };
    if (r.status === 400) return { state: "binary" };
    if (!r.ok) return { state: "offline" };
    const source = (await r.json()) as SourceSlice;
    return { state: "ok", source };
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    return { state: "offline" };
  }
}

/* ---- 仓库文件清单（GET /api/files，合同：按 path 升序，symbols=定义数） ---- */

export interface RepoFile {
  /** 仓库内相对路径 */
  path: string;
  /** 该文件内有行号的定义节点数 */
  symbols: number;
}

export type RepoFilesResult =
  | { state: "ok"; files: RepoFile[] }
  /** 404/501 —— 后端版本不支持该端点 */
  | { state: "unsupported" }
  | { state: "offline" };

export async function fetchRepoFiles(
  repo: string,
  signal?: AbortSignal,
): Promise<RepoFilesResult> {
  const params = new URLSearchParams({ repo });
  try {
    const r = await fetch(`${SERVER}/api/files?${params.toString()}`, {
      signal: signal ?? AbortSignal.timeout(6000),
    });
    if (r.status === 404 || r.status === 501) return { state: "unsupported" };
    if (!r.ok) return { state: "offline" };
    const body = (await r.json()) as { files?: RepoFile[] };
    return { state: "ok", files: body.files ?? [] };
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    return { state: "offline" };
  }
}

/* ---- 文件内符号（GET /api/file/symbols，合同：按 line 升序） ---- */

export interface FileSymbol {
  id: string;
  name: string;
  label: string;
  file: string;
  line: number;
  end_line: number;
}

export type FileSymbolsResult =
  | { state: "ok"; symbols: FileSymbol[] }
  /** 404/501 —— 后端版本不支持该端点 */
  | { state: "unsupported" }
  | { state: "offline" };

export async function fetchFileSymbols(
  repo: string,
  path: string,
  signal?: AbortSignal,
): Promise<FileSymbolsResult> {
  const params = new URLSearchParams({ repo, path });
  try {
    const r = await fetch(`${SERVER}/api/file/symbols?${params.toString()}`, {
      signal: signal ?? AbortSignal.timeout(6000),
    });
    if (r.status === 404 || r.status === 501) return { state: "unsupported" };
    if (!r.ok) return { state: "offline" };
    const body = (await r.json()) as { symbols?: FileSymbol[] };
    return { state: "ok", symbols: body.symbols ?? [] };
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    return { state: "offline" };
  }
}
