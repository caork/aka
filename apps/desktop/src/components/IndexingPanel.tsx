import { motion } from "framer-motion";
import { useState } from "react";
import {
  activeIndexPhase,
  buildIndexLogText,
  formatProgressCount,
  indexLogLines,
  indexPhaseLabel,
  INDEX_PHASES,
} from "../index-log";
import type { Repo } from "../store";

const spring = { type: "spring", stiffness: 300, damping: 30 } as const;

export default function IndexingPanel({ repo }: { repo: Repo }) {
  const [copied, setCopied] = useState(false);
  const progress = repo.progress;
  const activePhase = activeIndexPhase(repo);
  const phaseLabel = indexPhaseLabel(progress?.stage ?? repo.status);
  const progressCount = formatProgressCount(progress);
  const primaryMessage =
    repo.status === "failed"
      ? "Indexing failed"
      : progress?.message || "Indexing repository";
  const logs = indexLogLines(repo);
  const logText = buildIndexLogText(repo);

  const copyLogs = () => {
    void navigator.clipboard
      ?.writeText(logText)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      })
      .catch(() => undefined);
  };

  return (
    <div className="flex h-full items-center justify-center px-8 py-10" data-testid="indexing-panel">
      <motion.div
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={spring}
        className="w-full max-w-[760px]"
      >
        <div className="mb-4 flex items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="mb-1 flex items-center gap-2">
              <span className={`beacon ${repo.status}`} />
              <span className="text-[11px] font-semibold uppercase tracking-[0.08em] text-ink-3">
                {repo.status === "failed" ? "Indexing failed" : "Indexing repository"}
              </span>
            </div>
            <h2 className="truncate text-[18px] font-semibold text-ink">{repo.name}</h2>
            <p className="mono mt-1 truncate text-[11.5px] text-ink-3">{repo.path}</p>
          </div>
          <div className="text-right">
            <div className="text-[12px] font-semibold uppercase tracking-[0.08em] text-ink-3">
              {phaseLabel}
            </div>
            <div className="mono mt-1 text-[11px] text-ink-3">
              {progressCount || "active"}
            </div>
          </div>
        </div>

        <div className="mb-4">
          <div className="mb-3 grid grid-cols-6 gap-2">
            {INDEX_PHASES.map((phase, idx) => {
              const state =
                repo.status === "failed" && idx >= activePhase
                  ? "failed"
                  : idx < activePhase
                    ? "done"
                    : idx === activePhase
                      ? "active"
                      : "pending";
              return <PhaseStep key={phase.key} label={phase.label} state={state} />;
            })}
          </div>
          <div
            className={`index-activity ${repo.status === "failed" ? "failed" : ""}`}
            aria-hidden
          >
            <div />
          </div>
          <div className="mt-2 flex items-center justify-between gap-3 text-[12px]">
            <span className="min-w-0 truncate font-medium text-ink-2">{primaryMessage}</span>
            <span className="mono text-ink-3">{formatStage(progress?.stage ?? repo.status)}</span>
          </div>
          {typeof progress?.current === "number" && typeof progress?.total === "number" && (
            <div className="mt-1 mono text-[11px] text-ink-3">
              {progress.current.toLocaleString()} / {progress.total.toLocaleString()}
            </div>
          )}
        </div>

        <div className="mb-4 grid grid-cols-4 gap-2">
          <Stat label="Files" value={progress?.files ?? 0} />
          <Stat label="Nodes" value={progress?.nodes ?? repo.symbols} />
          <Stat label="Edges" value={progress?.edges ?? 0} />
          <Stat label="Chunks" value={progress?.chunks ?? 0} />
        </div>

        {repo.status === "failed" && repo.detail && (
          <div className="mb-4 rounded-[8px] bg-[var(--danger-fill)] px-3 py-2 text-[12px] leading-relaxed text-[var(--danger-ink)]">
            {repo.detail}
          </div>
        )}

        <div className="overflow-hidden rounded-[10px] bg-[var(--subtle-fill-2)] shadow-[inset_0_0_0_0.5px_var(--hairline)]">
          <div className="themed-divider flex items-center justify-between gap-3 border-b px-3 py-2">
            <span className="text-[11px] font-semibold uppercase tracking-[0.07em] text-ink-3">
              Logs
            </span>
            <button
              type="button"
              onClick={copyLogs}
              className="rounded-[6px] px-2 py-1 text-[11px] font-medium text-ink-3 transition hover:bg-[var(--subtle-fill)] hover:text-ink"
            >
              {copied ? "Copied" : "Copy"}
            </button>
          </div>
          <div className="scroll-area max-h-[220px] px-3 py-2">
            {logs.slice(-40).map((line, idx) => (
              <div key={`${idx}-${line}`} className="mono py-0.5 text-[11.5px] leading-relaxed text-ink-2">
                {line}
              </div>
            ))}
          </div>
        </div>
      </motion.div>
    </div>
  );
}

function PhaseStep({ label, state }: { label: string; state: "done" | "active" | "pending" | "failed" }) {
  return (
    <div
      className={`index-phase ${state}`}
      title={label}
      aria-label={`${label} ${state}`}
    >
      <span />
      <strong>{label}</strong>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-[8px] bg-[var(--subtle-fill)] px-3 py-2">
      <div className="tabular text-[14px] font-semibold text-ink">{value.toLocaleString()}</div>
      <div className="text-[10.5px] uppercase tracking-[0.06em] text-ink-3">{label}</div>
    </div>
  );
}

function formatStage(stage: string) {
  return stage.replace(/[_-]+/g, " ");
}
