import { motion } from "framer-motion";
import { useMemo } from "react";
import { mockSearch } from "../mock";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

export default function SearchView() {
  const query = useAppStore((s) => s.query);
  const results = useMemo(() => mockSearch(query), [query]);

  return (
    <div className="scroll-area h-full px-6 py-5" data-testid="search-view">
      <div className="mx-auto max-w-[760px]">
        <div className="mb-3 flex items-baseline justify-between">
          <h2 className="text-[13px] font-semibold text-ink-2">
            {query ? `Results for “${query}”` : "Top symbols"}
          </h2>
          <span className="tabular text-[12px] text-ink-3">
            {results.length} matches · 3.2 ms
          </span>
        </div>

        {results.length === 0 && (
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={spring}
            className="glass flex flex-col items-center gap-1.5 px-6 py-12 text-center"
          >
            <span className="text-[14px] font-medium text-ink">
              No matches
            </span>
            <span className="text-[12.5px] text-ink-3">
              Try a different identifier — fuzzy + BM25 search lands with the
              Rust core.
            </span>
          </motion.div>
        )}

        {results.map((r, idx) => (
          <motion.button
            key={r.id}
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ ...spring, delay: idx * 0.02 }}
            className="focus-ring glass group mb-2.5 block w-full px-4 py-3 text-left transition-shadow duration-150 ease-out hover:shadow-[inset_0_0_0_0.5px_rgba(15,23,42,0.06),0_0_0_1px_rgba(255,255,255,0.65),0_2px_6px_rgba(16,24,40,.05),0_16px_40px_-12px_rgba(16,24,40,.14)]"
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
