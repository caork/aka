/* Repo 管理 API —— 桌面端优先走 Tauri 内嵌后端；浏览器/dev 回退 HTTP aka serve。 */

import { asDesktopError, invokeDesktop, isDesktopRuntime } from "./desktop-api";
import { apiUrl, localServeUnavailable } from "./api-base";

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
  return localServeUnavailable();
}

async function postJson(path: string, body: unknown, timeout = 8000): Promise<void> {
  let r: Response;
  try {
    r = await fetch(apiUrl(path), {
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
    r = await fetch(apiUrl(path), {
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
  if (isDesktopRuntime()) {
    try {
      await invokeDesktop("import_repo", { request: input });
      return;
    } catch (e) {
      throw asDesktopError(e, "导入失败");
    }
  }
  await postJson("/api/repos/import", input);
}

export async function importZip(name: string, fileOrPath: File | string): Promise<void> {
  if (isDesktopRuntime()) {
    try {
      await invokeDesktop("import_repo_zip", {
        request: { name, path: String(fileOrPath) },
      });
      return;
    } catch (e) {
      throw asDesktopError(e, "导入失败");
    }
  }
  if (typeof fileOrPath === "string") {
    throw new Error("浏览器模式不支持直接读取本机 zip 路径");
  }
  const form = new FormData();
  form.append("name", name);
  form.append("file", fileOrPath);
  await postForm("/api/repos/import-zip", form);
}

export async function updateRepo(name: string): Promise<void> {
  if (isDesktopRuntime()) {
    try {
      await invokeDesktop("update_repo", { name });
      return;
    } catch (e) {
      throw asDesktopError(e, "更新失败");
    }
  }
  await postJson(`/api/repos/${encodeURIComponent(name)}/update`, {});
}

export async function updateZip(name: string, fileOrPath: File | string): Promise<void> {
  if (isDesktopRuntime()) {
    try {
      await invokeDesktop("update_repo_zip", { name, path: String(fileOrPath) });
      return;
    } catch (e) {
      throw asDesktopError(e, "上传失败");
    }
  }
  if (typeof fileOrPath === "string") {
    throw new Error("浏览器模式不支持直接读取本机 zip 路径");
  }
  const form = new FormData();
  form.append("file", fileOrPath);
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
  if (isDesktopRuntime()) {
    try {
      await invokeDesktop("set_repo_settings", {
        name,
        settings: {
          embeddingsEnabled: settings.embeddingsEnabled,
          renderMaxNodes: settings.renderMaxNodes,
        },
      });
      return;
    } catch (e) {
      throw asDesktopError(e, "设置失败");
    }
  }
  await postJson(`/api/repos/${encodeURIComponent(name)}/settings`, {
    embeddings_enabled: settings.embeddingsEnabled,
    render_max_nodes: settings.renderMaxNodes,
  });
}

export async function deleteRepo(name: string): Promise<void> {
  if (isDesktopRuntime()) {
    try {
      await invokeDesktop("delete_repo", { name });
      return;
    } catch (e) {
      throw asDesktopError(e, "移除失败");
    }
  }
  let r: Response;
  try {
    r = await fetch(apiUrl(`/api/repos/${encodeURIComponent(name)}`), {
      method: "DELETE",
      signal: AbortSignal.timeout(8000),
    });
  } catch (e) {
    throw asError(e);
  }
  await ensureOk(r);
}

export async function clearAppData(): Promise<void> {
  if (!isDesktopRuntime()) {
    throw new Error("清理应用数据仅支持桌面版");
  }
  try {
    await invokeDesktop("clear_app_data");
  } catch (e) {
    throw asDesktopError(e, "清理失败");
  }
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
    if (isDesktopRuntime()) {
      const detail = await invokeDesktop<NodeDetail>("node_detail", { repo, id });
      return { state: "ok", detail };
    }
    const r = await fetch(
      apiUrl(`/api/node?repo=${encodeURIComponent(repo)}&id=${encodeURIComponent(id)}`),
      { signal: signal ?? AbortSignal.timeout(4000) },
    );
    if (r.status === 404 || r.status === 501) return { state: "unsupported" };
    if (!r.ok) return { state: "offline" };
    const detail = (await r.json()) as NodeDetail;
    return { state: "ok", detail };
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    if (typeof e === "string" && e.includes("not supported")) {
      return { state: "unsupported" };
    }
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
  /** 文件清单来自旧索引或仓库工作区已变化，源码文件已不存在。 */
  | { state: "missing" }
  /** 桌面内置后端/HTTP 后端返回了可展示的读取错误。 */
  | { state: "error"; message: string }
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
  if (isDesktopRuntime()) {
    try {
      const source = await invokeDesktop<SourceSlice>("source", {
        repo,
        path,
        start,
        end,
      });
      return { state: "ok", source };
    } catch (e) {
      return sourceResultFromDesktopError(e);
    }
  }
  try {
    const r = await fetch(apiUrl(`/api/source?${params.toString()}`), {
      signal: signal ?? AbortSignal.timeout(6000),
    });
    if (r.status === 501) return { state: "unsupported" };
    if (r.status === 404) return await sourceResultFromHttpError(r);
    if (r.status === 400) return { state: "binary" };
    if (!r.ok) return await sourceResultFromHttpError(r);
    const source = (await r.json()) as SourceSlice;
    return { state: "ok", source };
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    return { state: "offline" };
  }
}

function sourceResultFromDesktopError(e: unknown): SourceResult {
  const message = desktopErrorMessage(e);
  if (message.includes("not supported")) return { state: "unsupported" };
  if (message.includes("invalid file")) return { state: "binary" };
  if (
    message.includes("file not found") ||
    message.includes("not a regular file") ||
    message.includes("repo not registered")
  ) {
    return { state: "missing" };
  }
  return { state: "error", message: conciseError(message) };
}

async function sourceResultFromHttpError(r: Response): Promise<SourceResult> {
  const message = await responseErrorMessage(r);
  if (message.includes("invalid file")) return { state: "binary" };
  if (message.includes("file not found") || message.includes("not a regular file")) {
    return { state: "missing" };
  }
  if (r.status >= 500) return { state: "error", message: conciseError(message) };
  return { state: "offline" };
}

function desktopErrorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

async function responseErrorMessage(r: Response): Promise<string> {
  try {
    const text = (await r.text()).trim();
    if (!text) return `HTTP ${r.status}`;
    try {
      const body = JSON.parse(text) as { error?: unknown };
      if (typeof body.error === "string" && body.error.trim()) return body.error;
    } catch {
      /* 非 JSON 错误体，直接展示文本摘要。 */
    }
    return text;
  } catch {
    return `HTTP ${r.status}`;
  }
}

function conciseError(message: string): string {
  return message.trim().replace(/\s+/g, " ").slice(0, 180) || "源码读取失败";
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
    if (isDesktopRuntime()) {
      const body = await invokeDesktop<{ files?: RepoFile[] }>("repo_files", { repo });
      return { state: "ok", files: body.files ?? [] };
    }
    const r = await fetch(apiUrl(`/api/files?${params.toString()}`), {
      signal: signal ?? AbortSignal.timeout(6000),
    });
    if (r.status === 404 || r.status === 501) return { state: "unsupported" };
    if (!r.ok) return { state: "offline" };
    const body = (await r.json()) as { files?: RepoFile[] };
    return { state: "ok", files: body.files ?? [] };
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    if (typeof e === "string" && e.includes("not supported")) {
      return { state: "unsupported" };
    }
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
    if (isDesktopRuntime()) {
      const body = await invokeDesktop<{ symbols?: FileSymbol[] }>("file_symbols", {
        repo,
        path,
      });
      return { state: "ok", symbols: body.symbols ?? [] };
    }
    const r = await fetch(apiUrl(`/api/file/symbols?${params.toString()}`), {
      signal: signal ?? AbortSignal.timeout(6000),
    });
    if (r.status === 404 || r.status === 501) return { state: "unsupported" };
    if (!r.ok) return { state: "offline" };
    const body = (await r.json()) as { symbols?: FileSymbol[] };
    return { state: "ok", symbols: body.symbols ?? [] };
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    if (typeof e === "string" && e.includes("not supported")) {
      return { state: "unsupported" };
    }
    return { state: "offline" };
  }
}
