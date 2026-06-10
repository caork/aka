import { motion } from "framer-motion";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

/** Placeholder — symbol 360° (callers / callees) lands with the Rust graph API. */
export default function SymbolView() {
  return (
    <div className="scroll-area h-full px-6 py-5" data-testid="symbol-view">
      <div className="mx-auto max-w-[980px]">
        <motion.div
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={spring}
          className="glass mb-4 flex items-center gap-3 px-5 py-4"
        >
          <span className="mono text-[15px] font-semibold text-ink">
            parse_artifact_stream
          </span>
          <span className="badge Function">Function</span>
          <span className="mono tabular ml-auto text-[11.5px] text-ink-3">
            crates/aka-core/src/ingest.rs:142
          </span>
        </motion.div>

        <div className="grid grid-cols-2 gap-4">
          {(["Callers", "Callees"] as const).map((title, col) => (
            <motion.section
              key={title}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ ...spring, delay: 0.05 + col * 0.04 }}
              className="glass-panel px-4 py-4"
            >
              <h3 className="mb-3 flex items-center justify-between px-1 text-[12px] font-semibold text-ink-2">
                {title}
                <span className="tabular text-[11px] font-normal text-ink-3">
                  awaiting graph API
                </span>
              </h3>
              {Array.from({ length: 5 }).map((_, i) => (
                <div
                  key={i}
                  className="mb-2 flex items-center gap-3 rounded-[10px] px-2 py-2"
                >
                  <span
                    className="h-[10px] w-[10px] flex-none rounded-full"
                    style={{ background: "rgba(15,23,42,0.07)" }}
                  />
                  <span
                    className="h-[11px] animate-pulse rounded-full"
                    style={{
                      width: `${52 - i * 6 + col * 8}%`,
                      background: "rgba(15,23,42,0.07)",
                      animationDelay: `${i * 120}ms`,
                    }}
                  />
                  <span
                    className="ml-auto h-[9px] w-[72px] animate-pulse rounded-full"
                    style={{
                      background: "rgba(15,23,42,0.05)",
                      animationDelay: `${i * 120 + 60}ms`,
                    }}
                  />
                </div>
              ))}
            </motion.section>
          ))}
        </div>
      </div>
    </div>
  );
}
