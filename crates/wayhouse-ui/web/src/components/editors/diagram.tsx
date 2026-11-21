import type { State } from "../ui/StatusMark";

/** The label must stop before the pool column; the full text is the tooltip and the table. */
export const clip = (t: string, max = 20) => (t.length > max ? `${t.slice(0, max - 1)}…` : t);

/** A round mark for up, a triangle for degraded, a square for down, an open circle for unknown: shape and colour, as in StatusMark. */
export function Mark({ x, y, state }: { x: number; y: number; state: State }) {
  if (state === "up") return <circle cx={x} cy={y} r={4.5} className="fill-good" />;
  if (state === "down") return <rect x={x - 4.5} y={y - 4.5} width={9} height={9} className="fill-bad" />;
  if (state === "degraded") return <path d={`M${x - 5.5} ${y + 4.5} L${x + 5.5} ${y + 4.5} L${x} ${y - 5} Z`} className="fill-warn" />;
  return <circle cx={x} cy={y} r={4} className="fill-none stroke-ink-muted" strokeWidth={1.5} />;
}

