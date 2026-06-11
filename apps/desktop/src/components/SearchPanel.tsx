import { motion } from "framer-motion";
import { useEffect, useState } from "react";
import { mockSearch } from "../mock";
import { runSearch } from "../search-api";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

/** compact=true: narrow sidebar list; compact=false (default): full-width centered panel */
export default function SearchPanel({ compact = false }: { compact?: boolean }) {
  const query = useAppStore((s) => s.query);
  const repoId = useAppStore((s) => s.selectedRepoId);
  const openDetail = useAppStore((s) => s.openDetail);
  const openCode = useAppStore((s) => s.openCode);
  const [results, setResults] = useState(() => mockSearch(""));
  const [tookMs, setTookMs] = useState(0);

  useEffect(() => {
    let stale = false;
    const t = window.setTimeout(() => {
      void runSearch(query, repoId || null).then((out) => {
        if (stale) return;
        setResults(out.results);
        setTookMs(out.tookMs);
      });
    }, 120);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [query, repoId]);

  if (compact) {
    return (
      <div className="scroll-area h-full px-2 pb-3 pt-14" data-testid="search-panel-compact">
        <div className="mb-2 flex items-center justify-between px-2">
          <span className="text-[11px] font-semibold uppercase tracking-[0.07em] text-ink-3">
            {query ? "Results" : "Top symbols"}
          </span>
          <span className="tabular text-[10.5px] text-ink-3">{results.length}</span>
        </div>

        {results.length === 0 && (
          <div className="px-2 py-6 text-center text-[12px] text-ink-3">No matches</div>
        )}

        {results.map((r, idx) => (
          <motion.button
            key={r.id}
            initial={{ opacity: 0, x: -4 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ ...spring, delay: idx * 0.015 }}
            onClick={() =>
              openCode({ repo: repoId, path: r.file, line: r.line })
            }
            className="focus-ring group mb-0.5 flex w-full flex-col gap-0.5 rounded-[8px] px-2 py-2 text-left transition-colors hover:bg-[rgba(15,23,42,0.05)]"
            data-testid="search-result"
          >
            <div className="flex min-w-0 items-center gap-1.5">
              <span className="truncate text-[12.5px] font-medium text-ink">
                <Highlight text={r.name} query={query} />
              </span>
              <span className={`badge ${r.label} flex-none text-[10px]`}>{r.label}</span>
            </div>
            <span className="mono truncate text-[10.5px] text-ink-3">
              {r.file}:{r.line}
            </span>
          </motion.button>
        ))}
      </div>
    );
  }

  /* full-width layout */
  return (
    <div className="scroll-area h-full px-6 pb-5 pt-14" data-testid="search-view">
      <div className="mx-auto max-w-[760px]">
        <div className="mb-3 flex items-baseline justify-between">
          <h2 className="text-[13px] font-semibold text-ink-2">
            {query ? `Results for "${query}"` : "Top symbols"}
          </h2>
          <span className="tabular text-[12px] text-ink-3">
            {results.length} matches · {tookMs.toFixed(1)} ms
          </span>
        </div>

        {results.length === 0 && (
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={spring}
            className="glass flex flex-col items-center gap-1.5 px-6 py-12 text-center"
          >
            <span className="text-[14px] font-medium text-ink">No matches</span>
            <span className="text-[12.5px] text-ink-3">
              Try a different identifier — fuzzy + BM25 search lands with the Rust core.
            </span>
          </motion.div>
        )}

        {results.map((r, idx) => (
          <motion.button
            key={r.id}
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ ...spring, delay: idx * 0.02 }}
            onClick={() => {
              openDetail({ id: r.id, name: r.name, label: r.label, file: r.file, line: r.line });
              openCode({ repo: repoId, path: r.file, line: r.line });
            }}
            className="focus-ring glass group mb-2.5 block w-full px-4 py-3 text-left transition-shadow duration-150 ease-out hover:shadow-[inset_0_0_0_0.5px_rgba(15,23,42,0.06),0_0_0_1px_rgba(255,255,255,0.65),0_2px_6px_rgba(16,24,40,.05),0_16px_40px_-12px_rgba(16,24,40,.14)]"
            data-testid="search-result"
          >
            <div className="flex items-center gap-2.5">
              <span className="text-[13.5px] font-semibold text-ink">
                <Highlight text={r.name} query={query} />
              </span>
              <span className={`badge ${r.label}`}>{r.label}</span>
              <span className="mono tabular ml-auto truncate pl-3 text-[11.5px] text-ink-3">
                {r.file}:{r.line}
              </span>
            </div>
            <div className="mono mt-2 truncate rounded-[8px] bg-[rgba(15,23,42,0.035)] px-3 py-1.5 text-[11.5px] leading-relaxed text-ink-2">
              <Highlight text={r.snippet} query={query} />
            </div>
          </motion.button>
        ))}
      </div>
    </div>
  );
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
