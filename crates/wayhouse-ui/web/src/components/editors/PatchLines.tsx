import { describeRoute, routeTarget, type Json } from "../../lib/editors/listeners";
import type { State } from "../ui/StatusMark";
import { StatusMark } from "../ui/StatusMark";
import { Table, TBody, TD, TH, THead, TR } from "../ui/Table";

export interface PatchListener {
  name: string;
  bind?: string;
  protocol?: string;
  /** The `pool` shorthand: one route that sends everything there. */
  pool?: string | null;
  routes?: Json[];
}

const ROW = 34;
const LEFT = 190;
const MID = 150;
const RIGHT = 170;
const W = LEFT + MID + RIGHT;

/** The label must stop before the pool column; the full text is the tooltip and the table. */
const clip = (t: string, max = 20) => (t.length > max ? `${t.slice(0, max - 1)}…` : t);

const WORD: Record<State, string> = { up: "up", degraded: "degraded", down: "down", unknown: "state unknown" };

/** A round mark for up, a triangle for degraded, a square for down, an open circle for unknown: shape and colour, as in StatusMark. */
function Mark({ x, y, state }: { x: number; y: number; state: State }) {
  if (state === "up") return <circle cx={x} cy={y} r={4.5} className="fill-good" />;
  if (state === "down") return <rect x={x - 4.5} y={y - 4.5} width={9} height={9} className="fill-bad" />;
  if (state === "degraded") return <path d={`M${x - 5.5} ${y + 4.5} L${x + 5.5} ${y + 4.5} L${x} ${y - 5} Z`} className="fill-warn" />;
  return <circle cx={x} cy={y} r={4} className="fill-none stroke-ink-muted" strokeWidth={1.5} />;
}

/**
 * The signature drawing (DESIGN.md): the listener on the left, one orthogonal patch line per route to the pool or
 * resolver it ends at on the right, a state mark at each end. `stateOf` gives a pool's live state. The same data is a
 * table underneath, and the drawing's label spells every route and state out.
 */
export function PatchLines({ listener, stateOf }: { listener: PatchListener; stateOf: (kind: "pool" | "resolver", name: string) => State }) {
  const routes: Json[] = listener.routes?.length ? listener.routes : listener.pool ? [{ match: { type: "always" }, action: { pool: listener.pool } }] : [];
  const rows = routes.map((r) => {
    const t = routeTarget(r);
    return { text: describeRoute(r), target: t, state: (t ? stateOf(t.kind, t.name) : "unknown") as State };
  });
  const height = Math.max(rows.length, 1) * ROW + 16;
  const mid = height / 2;
  const label = `${listener.name} ${listener.bind ?? ""} ${listener.protocol ?? ""}. ${
    rows.length ? rows.map((r, i) => `Route ${i + 1}: ${r.text}, ${WORD[r.state]}`).join(". ") : "No routes."
  }`;

  return (
    <figure className="mb-4">
      <div className="overflow-x-auto border border-line bg-surface p-2">
        <svg role="img" aria-label={label} width={W} height={height} viewBox={`0 0 ${W} ${height}`} className="min-w-[510px]">
          <Mark x={8} y={mid} state="up" />
          <text x={20} y={mid - 4} className="fill-ink text-xs font-mono">
            {listener.name}
          </text>
          <text x={20} y={mid + 11} className="fill-ink-muted text-xs font-mono">
            {[listener.bind, listener.protocol].filter(Boolean).join(" ")}
          </text>
          {rows.map((r, i) => {
            const y = 8 + ROW * i + ROW / 2;
            return (
              <g key={i}>
                <path d={`M${LEFT - 30} ${mid} H${LEFT + 20} V${y} H${LEFT + MID}`} className="fill-none stroke-control" strokeWidth={1.5} />
                <text x={LEFT + 26} y={y - 4} className="fill-ink-muted text-xs">
                  <title>{r.text}</title>
                  {clip(`${i + 1}. ${r.text.split(" to ")[0]}`)}
                </text>
                <Mark x={LEFT + MID + 6} y={y} state={r.state} />
                <text x={LEFT + MID + 18} y={y + 4} className="fill-ink text-xs font-mono">
                  {r.target ? `${r.target.kind === "resolver" ? "resolver " : ""}${r.target.name}` : "nothing yet"}
                </text>
              </g>
            );
          })}
        </svg>
      </div>
      <details className="mt-1 text-sm">
        <summary className="cursor-pointer text-ink-muted">View as table</summary>
        <Table aria-label={`Routes of ${listener.name}, as a table`}>
          <THead>
            <TR>
              <TH>Order</TH>
              <TH>Matches</TH>
              <TH>Goes to</TH>
              <TH>State</TH>
            </TR>
          </THead>
          <TBody>
            {rows.length === 0 && (
              <TR>
                <TD colSpan={4} className="text-ink-muted">
                  No routes: nothing is forwarded.
                </TD>
              </TR>
            )}
            {rows.map((r, i) => (
              <TR key={i}>
                <TD numeric>{i + 1}</TD>
                <TD>{r.text.split(" to ")[0]}</TD>
                <TD className="font-mono">{r.target ? `${r.target.kind} ${r.target.name}` : "nothing yet"}</TD>
                <TD>
                  <StatusMark state={r.state} word={WORD[r.state]} />
                </TD>
              </TR>
            ))}
          </TBody>
        </Table>
      </details>
    </figure>
  );
}
