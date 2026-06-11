import type { ContextRef } from "../search-api";

/**
 * 抽屉内嵌的 ego 小图谱（Code 模式的"连接"互补视图）。
 * 中心 = 当前符号；一圈邻居 = callers / callees / refs（1 跳，来自已拉取的 context）。
 * 点击邻居 → onPick（在 Code 视图里跳转中栏并重锚抽屉，图谱随之以新节点为中心刷新）。
 *
 * 纯 SVG、确定性布局，不引第二个 WebGL 实例。蓝=调用关系（箭头指向被调用方），
 * 灰=其它引用。无列/坐标依赖。
 */

const MAX_NEIGHBORS = 8;

type Group = "caller" | "callee" | "ref";

interface Placed {
  ref: ContextRef;
  group: Group;
  x: number;
  y: number;
}

/* viewBox 基准坐标 */
const VB_W = 336;
const VB_H = 240;
const CX = 168;
const CY = 112;
const RX = 120;
const RY = 80;
/* 中心 / 邻居 chip 半轴（用于把连线收进 chip 边缘外侧） */
const CHALF_X = 52;
const CHALF_Y = 15;
const NHALF_X = 40;
const NHALF_Y = 13;

const BLUE = "#2E7CF6";
const GRAY = "rgba(15,23,42,0.22)";

export default function EgoMiniGraph({
  centerName,
  callers,
  callees,
  refs,
  loading,
  onPick,
}: {
  centerName: string;
  callers: ContextRef[];
  callees: ContextRef[];
  refs: ContextRef[];
  loading: boolean;
  onPick(r: ContextRef): void;
}) {
  /* 唯一邻居：callees(出) → callers(入) → refs(其它)，去重，截断 */
  const seen = new Set<string>();
  const order: { ref: ContextRef; group: Group }[] = [];
  const add = (list: ContextRef[], group: Group) => {
    for (const r of list) {
      if (order.length >= MAX_NEIGHBORS) break;
      if (!r.id || seen.has(r.id)) continue;
      seen.add(r.id);
      order.push({ ref: r, group });
    }
  };
  add(callees, "callee");
  add(callers, "caller");
  add(refs, "ref");

  const totalUnique = new Set(
    [...callees, ...callers, ...refs].map((r) => r.id),
  ).size;
  const hidden = Math.max(0, totalUnique - order.length);

  const n = order.length;
  const placed: Placed[] = order.map((o, i) => {
    const ang = (-90 + (360 / Math.max(n, 1)) * i) * (Math.PI / 180);
    return {
      ...o,
      x: CX + RX * Math.cos(ang),
      y: CY + RY * Math.sin(ang),
    };
  });

  return (
    <div
      className="overflow-hidden rounded-[10px]"
      style={{
        background: "rgba(15,23,42,0.02)",
        boxShadow: "inset 0 0 0 0.5px rgba(15,23,42,0.07)",
      }}
      data-testid="ego-mini-graph"
    >
      {loading ? (
        <EgoSkeleton />
      ) : (
        <svg
          viewBox={`0 0 ${VB_W} ${VB_H}`}
          width="100%"
          style={{ height: "auto", display: "block" }}
          role="img"
          aria-label={`${centerName} 的图谱连接`}
        >
          <defs>
            <marker
              id="egoArrow"
              viewBox="0 0 10 10"
              refX="8.5"
              refY="5"
              markerWidth="6"
              markerHeight="6"
              orient="auto-start-reverse"
            >
              <path d="M0 0 L10 5 L0 10 z" fill={BLUE} opacity="0.7" />
            </marker>
            <style>{`
              .ego-chip { cursor: pointer; }
              .ego-chip rect { transition: fill .15s ease, stroke .15s ease; }
              .ego-chip:hover rect { fill: rgba(46,124,246,0.12); stroke: ${BLUE}; }
              .ego-chip:hover text { fill: #2563c9; }
            `}</style>
          </defs>

          {/* 连线（在 chip 之下） */}
          {placed.map((p) => {
            const ang = Math.atan2(p.y - CY, p.x - CX);
            const cos = Math.cos(ang);
            const sin = Math.sin(ang);
            const cEnd = { x: CX + CHALF_X * cos, y: CY + CHALF_Y * sin };
            const nEnd = { x: p.x - NHALF_X * cos, y: p.y - NHALF_Y * sin };
            const isCall = p.group !== "ref";
            /* caller：箭头指向中心（被调用方=中心）；callee：箭头指向邻居 */
            const path =
              p.group === "caller"
                ? `M ${nEnd.x} ${nEnd.y} L ${cEnd.x} ${cEnd.y}`
                : `M ${cEnd.x} ${cEnd.y} L ${nEnd.x} ${nEnd.y}`;
            return (
              <path
                key={`e-${p.ref.id}`}
                d={path}
                fill="none"
                stroke={isCall ? "rgba(46,124,246,0.5)" : GRAY}
                strokeWidth={1.4}
                strokeDasharray={p.group === "ref" ? "3 3" : undefined}
                markerEnd={isCall ? "url(#egoArrow)" : undefined}
              />
            );
          })}

          {/* 邻居 chip */}
          {placed.map((p) => (
            <g
              key={`n-${p.ref.id}`}
              className="ego-chip"
              onClick={() => onPick(p.ref)}
              role="button"
              tabIndex={0}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onPick(p.ref);
                }
              }}
            >
              <title>{`${p.ref.name} · ${groupLabel(p.group)}\n${p.ref.file}:${p.ref.line}`}</title>
              <rect
                x={p.x - NHALF_X}
                y={p.y - NHALF_Y}
                width={NHALF_X * 2}
                height={NHALF_Y * 2}
                rx={6}
                fill="#ffffff"
                stroke="rgba(15,23,42,0.14)"
                strokeWidth={1}
              />
              <circle
                cx={p.x - NHALF_X + 9}
                cy={p.y}
                r={2.6}
                fill={p.group === "ref" ? "rgba(15,23,42,0.3)" : BLUE}
              />
              <text
                x={p.x + 2}
                y={p.y + 3.5}
                textAnchor="middle"
                className="mono"
                fontSize="9.5"
                fill="#0f172a"
              >
                {truncate(p.ref.name, 9)}
              </text>
            </g>
          ))}

          {/* 中心 chip */}
          <g>
            <rect
              x={CX - CHALF_X}
              y={CY - CHALF_Y}
              width={CHALF_X * 2}
              height={CHALF_Y * 2}
              rx={8}
              fill={BLUE}
            />
            <text
              x={CX}
              y={CY + 3.5}
              textAnchor="middle"
              className="mono"
              fontSize="10.5"
              fontWeight="600"
              fill="#ffffff"
            >
              {truncate(centerName, 13)}
            </text>
          </g>
          </svg>
      )}

      {!loading && (
        <div className="flex items-center justify-between border-t border-[rgba(15,23,42,0.05)] px-2.5 py-1.5">
          <div className="flex items-center gap-3 text-[10px] text-ink-3">
            <span className="flex items-center gap-1">
              <span
                className="inline-block h-[2px] w-3 rounded"
                style={{ background: BLUE }}
              />
              调用
            </span>
            <span className="flex items-center gap-1">
              <span
                className="inline-block h-[2px] w-3 rounded"
                style={{
                  background:
                    "repeating-linear-gradient(90deg,rgba(15,23,42,0.3) 0 2px,transparent 2px 4px)",
                }}
              />
              引用
            </span>
          </div>
          {n === 0 ? (
            <span className="text-[10px] text-ink-3">暂无连接</span>
          ) : hidden > 0 ? (
            <span className="text-[10px] text-ink-3">+{hidden} 见下方列表</span>
          ) : null}
        </div>
      )}
    </div>
  );
}

function groupLabel(g: Group): string {
  return g === "caller" ? "调用方" : g === "callee" ? "被调用" : "引用";
}

function truncate(s: string, max: number): string {
  return s.length > max ? s.slice(0, max - 1) + "…" : s;
}

function EgoSkeleton() {
  return (
    <div
      className="flex items-center justify-center"
      style={{ height: 168 }}
      data-testid="ego-skeleton"
    >
      <div
        className="h-9 w-24 animate-pulse rounded-[8px]"
        style={{ background: "rgba(15,23,42,0.06)" }}
      />
    </div>
  );
}
