import { motion } from "framer-motion";
import type { ViewId } from "../store";

const VIEWS: { id: ViewId; label: string }[] = [
  { id: "code", label: "Code" },
  { id: "graph", label: "Graph" },
];

export default function SegmentedControl({
  value,
  onChange,
}: {
  value: ViewId;
  onChange(v: ViewId): void;
}) {
  return (
    <div
      className="segmented flex items-center gap-0.5 rounded-[10px] p-0.5"
      role="tablist"
      data-testid="view-switcher"
    >
      {VIEWS.map((v) => {
        const active = v.id === value;
        return (
          <button
            key={v.id}
            role="tab"
            aria-selected={active}
            onClick={() => onChange(v.id)}
            className="focus-ring relative rounded-[8px] px-3.5 py-1.5 text-[12.5px] font-medium transition-colors duration-150 ease-out"
            style={{ color: active ? "var(--ink)" : "var(--ink-2)" }}
          >
            {active && (
              <motion.span
                layoutId="segment-thumb"
                transition={{ type: "spring", stiffness: 400, damping: 32 }}
                className="segmented-thumb absolute inset-0 rounded-[8px]"
              />
            )}
            <span className="relative z-10">{v.label}</span>
          </button>
        );
      })}
    </div>
  );
}
