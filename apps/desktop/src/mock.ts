/** Mock search results until the tantivy-backed Tauri command lands. */

export interface SearchResult {
  id: string;
  name: string;
  label: "Function" | "Class" | "Method" | "Interface" | "File" | "Process";
  file: string;
  line: number;
  snippet: string;
  score: number;
}

export const MOCK_RESULTS: SearchResult[] = [
  {
    id: "1",
    name: "parse_artifact_stream",
    label: "Function",
    file: "crates/aka-core/src/ingest.rs",
    line: 142,
    snippet:
      "pub fn parse_artifact_stream(reader: impl BufRead) -> Result<ArtifactBatch> {",
    score: 14.2,
  },
  {
    id: "2",
    name: "GraphStore",
    label: "Class",
    file: "crates/aka-graph/src/store.rs",
    line: 38,
    snippet: "pub struct GraphStore { csr: CsrAdjacency, db: SqlitePool }",
    score: 12.8,
  },
  {
    id: "3",
    name: "search_references",
    label: "Function",
    file: "crates/aka-mcp/src/tools.rs",
    line: 305,
    snippet:
      "async fn search_references(&self, symbol: &str) -> McpResult<RefList> {",
    score: 11.9,
  },
  {
    id: "4",
    name: "query_hybrid",
    label: "Method",
    file: "crates/aka-search/src/hybrid.rs",
    line: 87,
    snippet:
      "fn query_hybrid(&self, q: &Query, k: usize) -> Vec<ScoredDoc> { // BM25 + RRF",
    score: 11.1,
  },
  {
    id: "5",
    name: "CsrAdjacency",
    label: "Class",
    file: "crates/aka-graph/src/csr.rs",
    line: 21,
    snippet: "pub struct CsrAdjacency { offsets: Vec<u32>, targets: Vec<u32> }",
    score: 10.4,
  },
  {
    id: "6",
    name: "ingest.rs",
    label: "File",
    file: "crates/aka-core/src/ingest.rs",
    line: 1,
    snippet: "//! NDJSON artifact ingestion — nodes / relationships / chunks.",
    score: 9.7,
  },
  {
    id: "7",
    name: "impact_radius",
    label: "Function",
    file: "crates/aka-graph/src/traverse.rs",
    line: 230,
    snippet:
      "pub fn impact_radius(&self, root: NodeId, depth: u8) -> ImpactSet {",
    score: 9.1,
  },
  {
    id: "8",
    name: "EmbeddingProvider",
    label: "Interface",
    file: "crates/aka-search/src/embed.rs",
    line: 14,
    snippet: "pub trait EmbeddingProvider { fn embed(&self, text: &str) -> Vec<f32>; }",
    score: 8.6,
  },
];

/** Mock 文件清单 —— `aka serve` 离线时让文件树有内容可渲染。 */
export interface MockFile {
  path: string;
  symbols: number;
}

export const MOCK_FILES: MockFile[] = [
  { path: "crates/aka-core/src/ingest.rs", symbols: 14 },
  { path: "crates/aka-core/src/types.rs", symbols: 22 },
  { path: "crates/aka-core/src/registry.rs", symbols: 9 },
  { path: "crates/aka-graph/src/store.rs", symbols: 31 },
  { path: "crates/aka-graph/src/csr.rs", symbols: 12 },
  { path: "crates/aka-graph/src/adjacency.rs", symbols: 17 },
  { path: "crates/aka-graph/src/traverse.rs", symbols: 20 },
  { path: "crates/aka-graph/src/layout.rs", symbols: 8 },
  { path: "crates/aka-search/src/hybrid.rs", symbols: 19 },
  { path: "crates/aka-search/src/embed.rs", symbols: 6 },
  { path: "crates/aka-search/src/tokenize.rs", symbols: 11 },
  { path: "crates/aka-mcp/src/tools.rs", symbols: 28 },
  { path: "crates/aka-mcp/src/ops.rs", symbols: 24 },
  { path: "crates/aka-server/src/lib.rs", symbols: 33 },
  { path: "apps/cli/src/main.rs", symbols: 7 },
  { path: "apps/cli/src/backend.rs", symbols: 26 },
  { path: "docs/contracts/artifacts.md", symbols: 0 },
  { path: "README.md", symbols: 0 },
];

export function mockFiles(): MockFile[] {
  return MOCK_FILES;
}

export function mockSearch(query: string): SearchResult[] {
  const q = query.trim().toLowerCase();
  if (!q) return MOCK_RESULTS;
  return MOCK_RESULTS.filter(
    (r) =>
      r.name.toLowerCase().includes(q) ||
      r.file.toLowerCase().includes(q) ||
      r.snippet.toLowerCase().includes(q),
  );
}
