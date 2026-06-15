import { motion } from "framer-motion";
import { useEffect, useState } from "react";
import {
  runSearch,
  type ProcessResult,
  type ProcessSymbolResult,
  type SearchResult,
} from "../search-api";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

/** compact=true: narrow sidebar list; compact=false (default): full-width centered panel */
export default function SearchPanel({ compact = false }: { compact?: boolean }) {
  const query = useAppStore((s) => s.query);
  const repoId = useAppStore((s) => s.selectedRepoId);
  const hasRepos = useAppStore((s) => s.repos.length > 0);
  const repoStatus = useAppStore(
    (s) => s.repos.find((r) => r.id === s.selectedRepoId)?.status ?? null,
  );
  const openDetail = useAppStore((s) => s.openDetail);
  const openCode = useAppStore((s) => s.openCode);
  const [results, setResults] = useState<SearchResult[]>([]);
  const [processes, setProcesses] = useState<ProcessResult[]>([]);
  const [definitions, setDefinitions] = useState<SearchResult[]>([]);
  const [tookMs, setTookMs] = useState(0);

  useEffect(() => {
    let stale = false;
    if (!hasRepos || repoStatus === "indexing" || repoStatus === "failed") {
      setResults([]);
      setProcesses([]);
      setDefinitions([]);
      setTookMs(0);
      return () => {
        stale = true;
      };
    }
    const t = window.setTimeout(() => {
      void runSearch(query, repoId || null).then((out) => {
        if (stale) return;
        setResults(out.results);
        setProcesses(out.processes);
        setDefinitions(out.definitions);
        setTookMs(out.tookMs);
      });
    }, 120);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [query, repoId, repoStatus, hasRepos]);

  if (compact) {
    return (
      <div className="scroll-area h-full px-2 pb-3 pt-3" data-testid="search-panel-compact">
        <div className="mb-2 flex items-center justify-between px-2">
          <span className="text-[11px] font-semibold uppercase tracking-[0.07em] text-ink-3">
            Results
          </span>
          <span className="tabular text-[10.5px] text-ink-3">
            {processes.length + results.length}
          </span>
        </div>

        {processes.length === 0 && results.length === 0 && (
          <div className="px-2 py-6 text-center text-[12px] text-ink-3">
            {hasRepos ? "No matches" : "No repositories"}
          </div>
        )}

        {processes.slice(0, 8).map((p, idx) => (
          <motion.button
            key={p.id}
            initial={{ opacity: 0, x: -4 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ ...spring, delay: idx * 0.015 }}
            onClick={() =>
              openDetail({ id: p.id, name: p.summary, label: "Process", file: "", line: 0 })
            }
            className="themed-hover focus-ring group mb-0.5 flex w-full flex-col gap-0.5 rounded-[8px] px-2 py-2 text-left transition-colors"
            data-testid="search-process-result"
          >
            <div className="flex min-w-0 items-center gap-1.5">
              <span className="truncate text-[12.5px] font-medium text-ink">
                <Highlight text={p.summary} query={query} />
              </span>
              <span className="badge Process flex-none text-[10px]">Process</span>
            </div>
            <span className="mono truncate text-[10.5px] text-ink-3">
              {p.symbolCount} symbols · {processTypeText(p.processType)}
            </span>
          </motion.button>
        ))}

        {results.map((r, idx) => (
          <motion.button
            key={r.id}
            initial={{ opacity: 0, x: -4 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ ...spring, delay: idx * 0.015 }}
            onClick={() =>
              /* 无源码位置的合成节点(Process 等)走详情面板,openCode 只会 404 */
              r.file
                ? openCode({ repo: repoId, path: r.file, line: r.line })
                : openDetail({ id: r.id, name: r.name, label: r.label, file: r.file, line: r.line })
            }
            className="themed-hover focus-ring group mb-0.5 flex w-full flex-col gap-0.5 rounded-[8px] px-2 py-2 text-left transition-colors"
            data-testid="search-result"
          >
            <div className="flex min-w-0 items-center gap-1.5">
              <span className="truncate text-[12.5px] font-medium text-ink">
                <Highlight text={r.name} query={query} />
              </span>
              <span className={`badge ${r.label} flex-none text-[10px]`}>{r.label}</span>
            </div>
            <span className="mono truncate text-[10.5px] text-ink-3">
              {r.file ? `${r.file}:${r.line}` : "执行流"}
            </span>
          </motion.button>
        ))}
      </div>
    );
  }

  /* full-width layout */
  return (
    <div className="scroll-area h-full px-6 pb-5 pt-3" data-testid="search-view">
      <div className="mx-auto max-w-[760px]">
        <div className="mb-3 flex items-baseline justify-between">
          <h2 className="text-[13px] font-semibold text-ink-2">
            {query ? `Results for "${query}"` : "Results"}
          </h2>
          <span className="tabular text-[12px] text-ink-3">
            {processes.length} flows · {results.length} matches · {tookMs.toFixed(1)} ms
          </span>
        </div>

        {processes.length === 0 && results.length === 0 && (
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={spring}
            className="glass flex flex-col items-center gap-1.5 px-6 py-12 text-center"
          >
            <span className="text-[14px] font-medium text-ink">No matches</span>
            <span className="text-[12.5px] text-ink-3">
              {hasRepos ? "Try a different identifier." : "Import a repository to start searching."}
            </span>
          </motion.div>
        )}

        {processes.length > 0 && (
          <section className="mb-5" aria-label="Flow results">
            <div className="mb-2 flex items-center justify-between px-1">
              <span className="text-[11px] font-semibold uppercase tracking-[0.07em] text-ink-3">
                Flow groups
              </span>
              <span className="tabular text-[11px] text-ink-3">{processes.length}</span>
            </div>
            {processes.map((p, idx) => (
              <motion.article
                key={p.id}
                initial={{ opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ ...spring, delay: idx * 0.025 }}
                className="glass mb-3 px-4 py-3"
                data-testid="search-process-group"
              >
                <button
                  type="button"
                  onClick={() =>
                    openDetail({ id: p.id, name: p.summary, label: "Process", file: "", line: 0 })
                  }
                  className="focus-ring themed-hover -mx-1 flex w-[calc(100%+0.5rem)] items-start gap-2 rounded-[8px] px-1 py-1 text-left transition-colors"
                >
                  <span className="badge Process mt-0.5 flex-none">Process</span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[13.5px] font-semibold text-ink">
                      <Highlight text={p.summary} query={query} />
                    </span>
                    <span className="mt-1 block text-[11.5px] text-ink-3">
                      {processTypeText(p.processType)}
                      {p.stepCount ? ` · ${p.stepCount} steps` : ""}
                      {` · ${p.symbolCount} matched symbols`}
                    </span>
                  </span>
                  <span className="mono tabular flex-none text-[11px] text-ink-3">
                    {p.priority.toFixed(1)}
                  </span>
                </button>

                {p.symbols.length > 0 && (
                  <div className="mt-2 space-y-1" data-testid="search-process-symbols">
                    {p.symbols.map((s) => (
                      <ProcessSymbolRow
                        key={`${p.id}-${s.id}`}
                        symbol={s}
                        query={query}
                        onOpen={() => {
                          openDetail({
                            id: s.id,
                            name: s.name,
                            label: s.label,
                            file: s.file,
                            line: s.line,
                          });
                          if (s.file) openCode({ repo: repoId, path: s.file, line: s.line });
                        }}
                      />
                    ))}
                  </div>
                )}
              </motion.article>
            ))}
          </section>
        )}

        {definitions.length > 0 && (
          <div className="mb-2 flex items-center justify-between px-1">
            <span className="text-[11px] font-semibold uppercase tracking-[0.07em] text-ink-3">
              Definitions
            </span>
            <span className="tabular text-[11px] text-ink-3">{definitions.length}</span>
          </div>
        )}

        {(definitions.length > 0 ? definitions : results).map((r, idx) => (
          <motion.button
            key={r.id}
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ ...spring, delay: idx * 0.02 }}
            onClick={() => {
              openDetail({ id: r.id, name: r.name, label: r.label, file: r.file, line: r.line });
              /* 合成节点无源码文件,源码 modal 只会 404 */
              if (r.file) openCode({ repo: repoId, path: r.file, line: r.line });
            }}
            className="focus-ring glass group mb-2.5 block w-full px-4 py-3 text-left transition-shadow duration-150 ease-out hover:shadow-[var(--shadow-panel)]"
            data-testid="search-result"
          >
            <div className="flex items-center gap-2.5">
              <span className="text-[13.5px] font-semibold text-ink">
                <Highlight text={r.name} query={query} />
              </span>
              <span className={`badge ${r.label}`}>{r.label}</span>
              <span className="mono tabular ml-auto truncate pl-3 text-[11.5px] text-ink-3">
                {r.file ? `${r.file}:${r.line}` : "执行流"}
              </span>
            </div>
            <div
              className="mono mt-2 truncate rounded-[8px] px-3 py-1.5 text-[11.5px] leading-relaxed text-ink-2"
              style={{ background: "var(--subtle-fill-2)" }}
            >
              <Highlight text={r.snippet} query={query} />
            </div>
          </motion.button>
        ))}
      </div>
    </div>
  );
}

function ProcessSymbolRow({
  symbol,
  query,
  onOpen,
}: {
  symbol: ProcessSymbolResult;
  query: string;
  onOpen(): void;
}) {
  return (
    <button
      type="button"
      onClick={onOpen}
      className="focus-ring themed-hover grid w-full grid-cols-[52px_minmax(0,1fr)_auto] items-center gap-2 rounded-[8px] px-2 py-1.5 text-left transition-colors"
      data-testid="search-process-symbol"
    >
      <span className="mono tabular text-[11px] text-ink-3">
        {symbol.stepIndex ? `#${symbol.stepIndex}` : "step"}
      </span>
      <span className="min-w-0">
        <span className="block truncate text-[12px] font-medium text-ink">
          <Highlight text={symbol.name} query={query} />
        </span>
        <span className="mono mt-0.5 block truncate text-[10.5px] text-ink-3">
          {symbol.file ? `${symbol.file}:${symbol.line}` : "执行流"}
          {symbol.module ? ` · ${symbol.module}` : ""}
        </span>
        {symbol.content && (
          <span className="mono mt-1 block truncate text-[10.5px] text-ink-2">
            <Highlight text={symbol.content} query={query} />
          </span>
        )}
      </span>
      <span className={`badge ${symbol.label} flex-none`}>{symbol.label}</span>
    </button>
  );
}

function processTypeText(t: string): string {
  switch (t) {
    case "cross_community":
      return "Cross-module flow";
    case "intra_community":
      return "Module-local flow";
    case "call-chain":
    case "call_chain":
      return "Call chain";
    default:
      return t.replace(/[_-]+/g, " ") || "Process";
  }
}

function Highlight({ text, query }: { text: string; query: string }) {
  const q = query.trim();
  if (!q) return <>{text}</>;
  const idx = text.toLowerCase().indexOf(q.toLowerCase());
  if (idx < 0) return <>{text}</>;
  return (
    <>
      {text.slice(0, idx)}
      <mark>{text.slice(idx, idx + q.length)}</mark>
      {text.slice(idx + q.length)}
    </>
  );
}
