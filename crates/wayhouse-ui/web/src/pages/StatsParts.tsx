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

/** Chart rows from a series: the open minute is flagged, a missing sample stays a gap. */
export const toRows = <P extends { t_ms: number; partial: boolean }>(points: P[], pick: (p: P) => Record<string, number | null>): ChartRow[] =>
  points.map((p) => ({ t: p.t_ms, partial: p.partial, ...pick(p) }));
