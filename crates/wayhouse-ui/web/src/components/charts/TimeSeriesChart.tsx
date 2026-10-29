import { useId, useState } from "react";
import { CartesianGrid, Line, LineChart, ReferenceDot, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { fmtClock } from "../../lib/stats";
import { Button } from "../ui/Button";
import { Table, TBody, TD, TH, THead, TR } from "../ui/Table";

export interface ChartSeries {
  key: string;
  label: string;
}
export type ChartRow = { t: number; partial?: boolean } & Record<string, number | boolean | null | undefined>;

/** Series colours in order (tokens.css), never state colours. */
const COLOUR = [1, 2, 3, 4, 5].map((i) => `var(--series-${i})`);

/**
 * A line chart that follows components.md "Charts": a zero baseline and real tick labels, a 1 px grid, series
 * from the token palette with a legend, a mark on the last point, an optional dashed onset line, a text
 * summary for assistive tech and the same data as a table.
 */
export function TimeSeriesChart({
  title,
  rows,
  series,
  format,
  onsetAt,
  onsetLabel,
  height = 200,
}: {
  title: string;
  rows: ChartRow[];
  series: ChartSeries[];
  format: (n: number) => string;
  /** Time of an event that explains a change; drawn as a dashed vertical line with `onsetLabel`. */
  onsetAt?: number;
  onsetLabel?: string;
  height?: number;
}) {
  const [asTable, setAsTable] = useState(false);
  const id = useId();
  const last = rows[rows.length - 1];
  const value = (r: ChartRow, k: string) => (typeof r[k] === "number" ? (r[k] as number) : undefined);
  const summary = last
    ? `${title}. At ${fmtClock(last.t)} UTC: ${series
        .map((s) => `${s.label} ${value(last, s.key) === undefined ? "no data" : format(value(last, s.key)!)}`)
        .join(", ")}.${onsetAt ? ` ${onsetLabel ?? "Change"} began at ${fmtClock(onsetAt)} UTC.` : ""}`
    : `${title}. No data in this range.`;

  return (
    <figure className="mb-6">
      <div className="mb-1 flex items-center justify-between gap-3">
        <figcaption className="text-sm font-semibold">{title}</figcaption>
        <Button size="sm" variant="ghost" aria-pressed={asTable} aria-controls={id} onClick={() => setAsTable(!asTable)}>
          {asTable ? "View as chart" : "View as table"}
        </Button>
      </div>
      <div id={id}>
        {asTable ? (
          <Table aria-label={`${title}, as a table`}>
            <THead>
              <TR>
                <TH>Time (UTC)</TH>
                {series.map((s) => (
                  <TH key={s.key} numeric>
                    {s.label}
                  </TH>
                ))}
              </TR>
            </THead>
            <TBody>
              {[...rows].reverse().map((r) => (
                <TR key={r.t}>
                  <TD className="tabular-nums">
                    {fmtClock(r.t)}
                    {r.partial && <span className="text-ink-muted"> (open minute)</span>}
                  </TD>
                  {series.map((s) => (
                    <TD key={s.key} numeric>
                      {value(r, s.key) === undefined ? "–" : format(value(r, s.key)!)}
                    </TD>
                  ))}
                </TR>
              ))}
            </TBody>
          </Table>
        ) : rows.length === 0 ? (
          <p role="img" aria-label={summary} className="py-6 text-sm text-ink-muted">
            No data in this range.
          </p>
        ) : (
          <div role="img" aria-label={summary} style={{ height }}>
            <ResponsiveContainer width="100%" height="100%">
              <LineChart data={rows} margin={{ top: 8, right: 12, bottom: 0, left: 0 }}>
                <CartesianGrid stroke="var(--line)" strokeWidth={1} vertical={false} />
                <XAxis dataKey="t" tickFormatter={fmtClock} stroke="var(--ink-muted)" tick={{ fontSize: 12 }} tickLine={false} minTickGap={32} />
                <YAxis domain={[0, "auto"]} tickFormatter={format} stroke="var(--ink-muted)" tick={{ fontSize: 12 }} tickLine={false} width={64} />
                <Tooltip
                  labelFormatter={(t) => `${fmtClock(Number(t))} UTC`}
                  formatter={(v) => (typeof v === "number" ? format(v) : v)}
                  contentStyle={{ background: "var(--surface)", border: "1px solid var(--line)", borderRadius: 3, fontSize: 12 }}
                />
                {onsetAt !== undefined && (
                  <ReferenceLine x={onsetAt} stroke="var(--ink-muted)" strokeDasharray="4 3" label={{ value: onsetLabel, fontSize: 12, fill: "var(--ink-muted)", position: "insideTopLeft" }} />
                )}
                {series.map((s, i) => (
                  <Line key={s.key} dataKey={s.key} name={s.label} type="linear" stroke={COLOUR[i % COLOUR.length]} strokeWidth={1.5} dot={false} isAnimationActive={false} />
                ))}
                {series.map((s, i) =>
                  value(last, s.key) === undefined ? null : (
                    <ReferenceDot key={s.key} x={last.t} y={value(last, s.key)} r={3} fill={COLOUR[i % COLOUR.length]} stroke="var(--surface)" />
                  ),
                )}
              </LineChart>
            </ResponsiveContainer>
          </div>
        )}
      </div>
      {!asTable && rows.length > 0 && (
        <ul className="mt-1 flex flex-wrap gap-x-4 text-xs text-ink-muted" aria-hidden>
          {series.map((s, i) => (
            <li key={s.key} className="flex items-center gap-1.5">
              <span className="inline-block h-0.5 w-4" style={{ background: COLOUR[i % COLOUR.length] }} />
              {s.label}
            </li>
          ))}
        </ul>
      )}
    </figure>
  );
}
