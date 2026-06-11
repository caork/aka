import { motion } from "framer-motion";
import type { ViewId } from "../store";

const VIEWS: { id: ViewId; label: string }[] = [
  { id: "doc", label: "Doc" },
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
      className="flex items-center gap-0.5 rounded-[10px] p-0.5"
      style={{
        background: "rgba(15,23,42,0.05)",
        boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.06)",
      }}
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
            style={{ color: active ? "#0f172a" : "#475569" }}
          >
            {active && (
              <motion.span
                layoutId="segment-thumb"
                transition={{ type: "spring", stiffness: 400, damping: 32 }}
                className="absolute inset-0 rounded-[8px] bg-white"
                style={{
                  boxShadow:
                    "0 1px 2px rgba(16,24,40,.06), 0 2px 6px rgba(16,24,40,.06), inset 0 0 0 0.5px rgba(15,23,42,.04)",
                }}
              />
            )}
            <span className="relative z-10">{v.label}</span>
          </button>
        );
      })}
    </div>
  );
}
