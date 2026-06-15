/* Real search — POST /api/query against the embedded desktop backend or aka serve. */

import { invokeDesktop, isDesktopRuntime } from "./desktop-api";
import { apiUrl } from "./api-base";

interface HitOut {
  id: string;
  name: string;
  label: string;
  file: string;
  line: number;
  score: number;
  snip?: string | null;
  processes?: string[] | null;
}

interface QueryProcessOut {
  id: string;
  summary: string;
  priority: number;
  symbol_count: number;
  process_type: string;
  step_count?: number | null;
}

interface QueryProcessSymbolOut {
  id: string;
  name: string;
  type: string;
  filePath: string;
  startLine: number;
  score: number;
  process_id: string;
  step_index?: number | null;
  module?: string | null;
  content?: string | null;
}

interface QueryOut {
  hits?: HitOut[];
  processes?: QueryProcessOut[];
  process_symbols?: QueryProcessSymbolOut[];
  definitions?: HitOut[];
}

export interface SearchResult {
  id: string;
  name: string;
  label: "Function" | "Class" | "Method" | "Interface" | "File" | "Process";
  file: string;
  line: number;
  snippet: string;
  score: number;
  processes: string[];
}

export interface ProcessSymbolResult {
  id: string;
  name: string;
  label: SearchResult["label"];
  file: string;
  line: number;
  score: number;
  stepIndex: number | null;
  module: string | null;
  content: string;
}

export interface ProcessResult {
  id: string;
  summary: string;
  processType: string;
  priority: number;
  symbolCount: number;
  stepCount: number | null;
  symbols: ProcessSymbolResult[];
}

const KNOWN_LABELS = new Set([
  "Function",
  "Class",
  "Method",
  "Interface",
  "File",
  "Process",
]);

function stripBold(s: string): string {
  return s.replace(/<\/?b>/g, "").replace(/&#x27;/g, "'").replace(/&amp;/g, "&");
}

export interface SearchOutcome {
  results: SearchResult[];
  processes: ProcessResult[];
  definitions: SearchResult[];
  tookMs: number;
}

export async function runSearch(
  query: string,
  repo: string | null,
): Promise<SearchOutcome> {
  const t0 = performance.now();
  try {
    const body = isDesktopRuntime()
      ? await invokeDesktop<QueryOut>("query", {
          query: query.trim() || "a",
          repo: repo ?? undefined,
          limit: 30,
        })
      : await runSearchHttp(query, repo);
    const processSymbols = new Map<string, ProcessSymbolResult[]>();
    for (const s of body.process_symbols ?? []) {
      const list = processSymbols.get(s.process_id) ?? [];
      list.push(mapProcessSymbol(s));
      processSymbols.set(s.process_id, list);
    }
    const processes: ProcessResult[] = (body.processes ?? []).map((p) => ({
      id: p.id,
      summary: p.summary || p.id,
      processType: p.process_type || "process",
      priority: p.priority,
      symbolCount: p.symbol_count,
      stepCount: p.step_count ?? null,
      symbols: processSymbols.get(p.id) ?? [],
    }));
    const results = (body.hits ?? []).map(mapHit);
    const definitions = (body.definitions ?? []).map(mapHit);
    return { results, processes, definitions, tookMs: performance.now() - t0 };
  } catch {
    return {
      results: [],
      processes: [],
      definitions: [],
      tookMs: performance.now() - t0,
    };
  }
}

async function runSearchHttp(
  query: string,
  repo: string | null,
): Promise<QueryOut> {
  const r = await fetch(apiUrl("/api/query"), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      query: query.trim() || "a",
      repo: repo ?? undefined,
      limit: 30,
    }),
    signal: AbortSignal.timeout(2500),
  });
  if (!r.ok) throw new Error(String(r.status));
  return (await r.json()) as QueryOut;
}

function mapHit(h: HitOut): SearchResult {
  return {
    id: h.id,
    name: h.name || h.file.split("/").pop() || h.id,
    label: normalizeLabel(h.label),
    file: h.file,
    line: h.line,
    snippet: stripBold(h.snip ?? ""),
    score: h.score,
    processes: h.processes ?? [],
  };
}

function mapProcessSymbol(s: QueryProcessSymbolOut): ProcessSymbolResult {
  return {
    id: s.id,
    name: s.name || s.filePath.split("/").pop() || s.id,
    label: normalizeLabel(s.type),
    file: s.filePath,
    line: s.startLine,
    score: s.score,
    stepIndex: s.step_index ?? null,
    module: s.module ?? null,
    content: stripBold(s.content ?? ""),
  };
}

function normalizeLabel(label: string): SearchResult["label"] {
  return (KNOWN_LABELS.has(label) ? label : "Function") as SearchResult["label"];
}

/* ---- Symbol 360° 上下文（POST /api/symbol/context）——
   SymbolView 与 DetailPanel 共用。 ---- */

export interface ContextHit {
  id: string;
  name: string;
  label: string;
  file: string;
  line: number;
}

export interface ContextRef extends ContextHit {
  edge: string;
  depth: number;
}

export interface SymbolContext {
  symbol: string;
  defs: ContextHit[];
  callers: ContextRef[];
  callees: ContextRef[];
  refs: ContextRef[];
}

/** null = HTTP 非 2xx（符号未找到/后端不支持）；"offline" = 服务不可达。 */
export type SymbolContextResult = SymbolContext | "offline" | null;

/** 拉取符号 360° 上下文，按离线/未找到优雅降级（绝不抛错，Abort 除外）。 */
export async function fetchSymbolContext(
  symbol: string,
  repo: string | null,
  signal?: AbortSignal,
): Promise<SymbolContextResult> {
  try {
    if (isDesktopRuntime()) {
      return await invokeDesktop<SymbolContext>("symbol_context", {
        symbol,
        repo: repo ?? undefined,
      });
    }
    const r = await fetch(apiUrl("/api/symbol/context"), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ symbol, repo: repo ?? undefined }),
      signal: signal ?? AbortSignal.timeout(4000),
    });
    if (!r.ok) return null;
    return (await r.json()) as SymbolContext;
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    return "offline";
  }
}
