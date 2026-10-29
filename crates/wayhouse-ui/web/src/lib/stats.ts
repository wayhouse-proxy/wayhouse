import { useQuery } from "@tanstack/react-query";
import { getFleetStats } from "../api";

export type Range = "5m" | "15m" | "1h";
export const RANGES: { value: Range; label: string }[] = [
  { value: "5m", label: "Last 5 minutes" },
  { value: "15m", label: "Last 15 minutes" },
  { value: "1h", label: "Last 60 minutes" },
];
export const parseRange = (s: string | null): Range => (RANGES.some((r) => r.value === s) ? (s as Range) : "15m");

export interface Series<P> {
  resolution_sec: number;
  last_sample_ms: number | null;
  points: P[];
}
export type Named<P> = Series<P> & { name: string };
interface Point {
  t_ms: number;
  /** The open minute: its rates cover only the time elapsed so far. */
  partial: boolean;
}
export interface FleetPoint extends Point {
  accepted_per_sec: number;
  blocked_per_sec: number;
  bytes_per_sec: number;
  packets_per_sec: number;
  tcp_sessions: number | null;
  udp_sessions: number | null;
}
export interface ListenerPoint extends Point {
  accepted_per_sec: number;
  blocked_per_sec: number;
  bytes_c2s_per_sec: number;
  bytes_s2c_per_sec: number;
  packets_per_sec: number;
}
export interface PoolPoint extends Point {
  healthy_backends: number;
  backends: number;
}
/** `GET /fleet/stats` (aggregator `history.rs`): aggregates only, never a per-instance series. */
export interface FleetStats {
  generated_at_ms: number;
  range_sec: number;
  fleet: Series<FleetPoint>;
  listeners: Named<ListenerPoint>[];
  pools: Named<PoolPoint>[];
  regions: Named<Point & { bytes_per_sec: number }>[];
}

/** The rolling history changes once a minute; polling every 30 s keeps the open minute current. */
export function useFleetStats(range: Range) {
  return useQuery({ queryKey: ["fleet-stats", range], queryFn: () => getFleetStats(range), refetchInterval: 30_000 });
}

const trim = (n: number) => {
  const s = n >= 100 ? n.toFixed(0) : n >= 10 ? n.toFixed(1) : n.toFixed(2);
  return s.includes(".") ? s.replace(/\.?0+$/, "") : s;
};
export const fmtCount = (n: number) => Math.round(n).toLocaleString("en-US");
export const fmtRate = (n: number) => `${trim(n)}/s`;
export function fmtBytes(n: number): string {
  const units = ["B/s", "kB/s", "MB/s", "GB/s", "TB/s"];
  let i = 0;
  while (n >= 1000 && i < units.length - 1) (n /= 1000), i++;
  return `${trim(n)} ${units[i]}`;
}
export const fmtClock = (ms: number) => new Date(ms).toISOString().slice(11, 16);

/** The start of the most recent run of non-zero values that follows a zero: "when did it begin". */
export function onset<P>(points: P[], value: (p: P) => number, time: (p: P) => number): number | undefined {
  for (let i = points.length - 1; i > 0; i--) if (value(points[i]) > 0 && value(points[i - 1]) === 0) return time(points[i]);
  return undefined;
}

/** "No data", "Last sample 40 s ago" or stale: freshness is stated, never implied. */
export function freshness(generatedAt: number, lastSample: number | null): { text: string; stale: boolean } {
  if (lastSample === null) return { text: "No instance has reported in this range", stale: true };
  const age = Math.max(0, Math.round((generatedAt - lastSample) / 1000));
  const text = age < 90 ? `Last sample ${age} s ago` : `Last sample ${Math.round(age / 60)} min ago`;
  return { text, stale: age > 180 };
}
