import { useSearchParams } from "react-router-dom";
import { freshness, parseRange, RANGES, type FleetStats, type Range } from "../lib/stats";
import { Select } from "../components/ui/Input";
import type { ChartRow } from "../components/charts/TimeSeriesChart";

/** The range lives in the URL so a link shows the same window. */
export function useRange(): [Range, (r: Range) => void] {
  const [params, setParams] = useSearchParams();
  return [parseRange(params.get("range")), (r) => setParams((p) => (p.set("range", r), p), { replace: true })];
}

export function RangeSelect({ value, onChange }: { value: Range; onChange: (r: Range) => void }) {
  return (
    <Select aria-label="Time range" value={value} onChange={(e) => onChange(e.target.value as Range)}>
      {RANGES.map((r) => (
        <option key={r.value} value={r.value}>
          {r.label}
        </option>
      ))}
    </Select>
  );
}

export const freshnessText = (s: FleetStats, last: number | null) => freshness(s.generated_at_ms, last);

/**
 * Chart rows from a series: the open minute is flagged, and every minute between the first and the last
 * sample gets a row (null values for a minute nobody reported), so the chart and its table show the gap
 * instead of joining the line across it.
 */
export const toRows = <P extends { t_ms: number; partial: boolean }>(points: P[], pick: (p: P) => Record<string, number | null>, stepMs = 60_000): ChartRow[] => {
  const rows: ChartRow[] = [];
  for (const p of points) {
    const picked = pick(p);
    const prev = rows[rows.length - 1];
    if (prev) for (let t = (prev.t as number) + stepMs; t < p.t_ms; t += stepMs) rows.push({ t, partial: false, ...Object.fromEntries(Object.keys(picked).map((k) => [k, null])) });
    rows.push({ t: p.t_ms, partial: p.partial, ...picked });
  }
  return rows;
};
