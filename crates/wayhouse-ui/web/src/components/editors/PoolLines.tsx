import type { State } from "../ui/StatusMark";
import { StatusMark } from "../ui/StatusMark";
import { Table, TBody, TD, TH, THead, TR } from "../ui/Table";

export interface BackendRow {
  addr: string;
  state: State;
}

const ROW = 28;
const W = 440;
const WORD: Record<State, string> = { up: "up", degraded: "degraded", down: "down", unknown: "state unknown" };

function Mark({ x, y, state }: { x: number; y: number; state: State }) {
  if (state === "up") return <circle cx={x} cy={y} r={4.5} className="fill-good" />;
  if (state === "down") return <rect x={x - 4.5} y={y - 4.5} width={9} height={9} className="fill-bad" />;
  if (state === "degraded") return <path d={`M${x - 5.5} ${y + 4.5} L${x + 5.5} ${y + 4.5} L${x} ${y - 5} Z`} className="fill-warn" />;
  return <circle cx={x} cy={y} r={4} className="fill-none stroke-ink-muted" strokeWidth={1.5} />;
}

/**
 * The pool's patch lines (DESIGN.md): the pool on the left, one line to each backend on the right with its live state
 * (across every instance that reports the pool). The drawing's label and a table underneath carry the same facts.
 */
export function PoolLines({ name, source, rows }: { name: string; source?: string; rows: BackendRow[] }) {
  const height = Math.max(rows.length, 1) * ROW + 16;
  const mid = height / 2;
  const label = `Pool ${name}${source ? `, backends from source ${source}` : ""}. ${
    rows.length ? rows.map((r, i) => `Backend ${i + 1}: ${r.addr}, ${WORD[r.state]}`).join(". ") : "No backends."
  }`;
  return (
    <figure className="mb-4">
      <div className="overflow-x-auto border border-line bg-surface p-2">
        <svg role="img" aria-label={label} width={W} height={height} viewBox={`0 0 ${W} ${height}`} className="min-w-[380px]">
          <text x={8} y={mid + 4} className="fill-ink text-xs font-mono">
            {name.length > 18 ? `${name.slice(0, 17)}…` : name}
          </text>
          {rows.map((r, i) => {
            const y = 8 + ROW * i + ROW / 2;
            return (
              <g key={i}>
                <path d={`M150 ${mid} H200 V${y} H230`} className="fill-none stroke-control" strokeWidth={1.5} />
                <Mark x={238} y={y} state={r.state} />
                <text x={252} y={y + 4} className="fill-ink text-xs font-mono">
                  {r.addr}
                </text>
              </g>
            );
          })}
        </svg>
      </div>
      <details className="mt-1 text-sm">
        <summary className="cursor-pointer text-ink-muted">View as table</summary>
        <Table aria-label={`Backends of ${name}, as a table`}>
          <THead>
            <TR>
              <TH>Backend</TH>
              <TH>State</TH>
            </TR>
          </THead>
          <TBody>
            {rows.length === 0 && (
              <TR>
                <TD colSpan={2} className="text-ink-muted">
                  {source ? "No backend has been discovered yet." : "No backends: nothing is forwarded."}
                </TD>
              </TR>
            )}
            {rows.map((r, i) => (
              <TR key={i}>
                <TD className="font-mono">{r.addr}</TD>
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
