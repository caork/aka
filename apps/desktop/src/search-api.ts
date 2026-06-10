/* Real search — POST /api/query against a local `aka serve`.
   Falls back to the mock dataset when the server is offline. */

import { mockSearch, type SearchResult } from "./mock";

const SERVER = "http://127.0.0.1:4111";

interface HitOut {
  id: string;
  name: string;
  label: string;
  file: string;
  line: number;
  score: number;
  snip?: string | null;
}

const KNOWN_LABELS = new Set([
  "Function",
  "Class",
  "Method",
  "Interface",
  "File",
]);

function stripBold(s: string): string {
  return s.replace(/<\/?b>/g, "").replace(/&#x27;/g, "'").replace(/&amp;/g, "&");
}

export interface SearchOutcome {
  results: SearchResult[];
  tookMs: number;
  live: boolean;
}

export async function runSearch(
  query: string,
  repo: string | null,
): Promise<SearchOutcome> {
  const t0 = performance.now();
  try {
    const r = await fetch(`${SERVER}/api/query`, {
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
    const body = (await r.json()) as { hits?: HitOut[] };
    const results: SearchResult[] = (body.hits ?? []).map((h) => ({
      id: h.id,
      name: h.name || h.file.split("/").pop() || h.id,
      label: (KNOWN_LABELS.has(h.label)
        ? h.label
        : "Function") as SearchResult["label"],
      file: h.file,
      line: h.line,
      snippet: stripBold(h.snip ?? ""),
      score: h.score,
    }));
    return { results, tookMs: performance.now() - t0, live: true };
  } catch {
    return {
      results: mockSearch(query),
      tookMs: performance.now() - t0,
      live: false,
    };
  }
}
