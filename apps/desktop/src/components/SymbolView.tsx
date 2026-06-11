import { motion } from "framer-motion";
import { useEffect, useState } from "react";
import {
  fetchSymbolContext,
  type ContextHit,
  type ContextRef,
  type SymbolContext,
} from "../search-api";
import { useAppStore } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

/** Symbol 360° — definition + callers / callees / references via `aka serve`. */
export default function SymbolView() {
  const query = useAppStore((s) => s.query);
  const repoId = useAppStore((s) => s.selectedRepoId);
  const openDetail = useAppStore((s) => s.openDetail);
  const symbol = query.trim().split(/\s+/)[0] || "";
  const [ctx, setCtx] = useState<SymbolContext | null>(null);
  const [state, setState] = useState<"idle" | "loading" | "offline">("idle");

  useEffect(() => {
    if (!symbol) {
      setCtx(null);
      setState("idle");
      return;
    }
    let stale = false;
    setState("loading");
    const t = window.setTimeout(() => {
      fetchSymbolContext(symbol, repoId || null)
        .then((out) => {
          if (stale) return;
          if (out === "offline") {
            setCtx(null);
            setState("offline");
          } else {
            setCtx(out);
            setState("idle");
          }
        })
        .catch(() => {
          if (stale) return;
          setCtx(null);
          setState("offline");
        });
    }, 150);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [symbol, repoId]);

  const def = ctx?.defs?.[0];

  /* 点击条目 → 右侧 DetailPanel 预览 */
  const pick = (r: ContextHit) => {
    openDetail({
      id: r.id,
      name: r.name,
      label: r.label,
      file: r.file,
      line: r.line,
    });
  };

  return (
    <div className="scroll-area h-full px-6 py-5" data-testid="symbol-view">
      <div className="mx-auto max-w-[980px]">
        <motion.div
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={spring}
          className="glass mb-4 flex items-center gap-3 px-5 py-4"
        >
          {def ? (
            <>
              <span className="mono text-[15px] font-semibold text-ink">
                {def.name}
              </span>
              <span className={`badge ${def.label}`}>{def.label}</span>
              <span className="mono tabular ml-auto text-[11.5px] text-ink-3">
                {def.file}:{def.line}
              </span>
            </>
          ) : (
            <span className="text-[13px] text-ink-3">
              {state === "offline"
                ? "aka serve 未在线 — 启动后即可查看符号 360°"
                : symbol
                  ? state === "loading"
                    ? `解析 ${symbol} …`
                    : `未找到符号 “${symbol}”`
                  : "在上方搜索框输入符号名（如 createUser），切到本视图查看 360° 上下文"}
            </span>
          )}
        </motion.div>

        <div className="grid grid-cols-2 gap-4">
          {(
            [
              ["Callers", ctx?.callers ?? []],
              ["Callees", ctx?.callees ?? []],
            ] as const
          ).map(([title, rows], col) => (
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
                  {rows.length}
                </span>
              </h3>
              {rows.length === 0 && (
                <div className="px-2 py-3 text-[12px] text-ink-3">—</div>
              )}
              {rows.slice(0, 30).map((r: ContextRef, i) => (
                <motion.button
                  key={`${r.id}-${i}`}
                  initial={{ opacity: 0, y: 6 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ ...spring, delay: i * 0.02 }}
                  onClick={() => pick(r)}
                  className="themed-hover focus-ring mb-1 flex w-full items-center gap-3 rounded-[10px] px-2 py-2 text-left transition-colors duration-150"
                  data-testid="symbol-relation-row"
                >
                  <span
                    className="h-[8px] w-[8px] flex-none rounded-full"
                    style={{
                      background:
                        r.depth <= 1
                          ? "var(--accent)"
                          : "color-mix(in srgb, var(--accent) 42%, transparent)",
                    }}
                  />
                  <span className="mono truncate text-[12.5px] text-ink">
                    {r.name}
                  </span>
                  <span className="mono tabular ml-auto flex-none text-[11px] text-ink-3">
                    {r.file.split("/").pop()}:{r.line}
                  </span>
                </motion.button>
              ))}
            </motion.section>
          ))}
        </div>

        {ctx && ctx.refs.length > 0 && (
          <motion.section
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ ...spring, delay: 0.12 }}
            className="glass-panel mt-4 px-4 py-4"
          >
            <h3 className="mb-3 px-1 text-[12px] font-semibold text-ink-2">
              References
            </h3>
            <div className="grid grid-cols-2 gap-x-6">
              {ctx.refs.slice(0, 20).map((r: ContextRef, i) => (
                <button
                  key={`${r.id}-${i}`}
                  onClick={() => pick(r)}
                  className="themed-hover focus-ring flex w-full items-center gap-2.5 rounded-[10px] px-2 py-1.5 text-left transition-colors duration-150"
                  data-testid="symbol-ref-row"
                >
                  <span className="badge Function flex-none text-[10px]">
                    {r.edge}
                  </span>
                  <span className="mono truncate text-[12px] text-ink-2">
                    {r.name}
                  </span>
                  <span className="mono tabular ml-auto flex-none text-[11px] text-ink-3">
                    {r.file.split("/").pop()}:{r.line}
                  </span>
                </button>
              ))}
            </div>
          </motion.section>
        )}
      </div>
    </div>
  );
}
