import { ErrorState, Loading, EmptyState } from "../components/shell/States";
import { PageHeader } from "../components/shell/Page";
import { TimeSeriesChart } from "../components/charts/TimeSeriesChart";
import { fmtBytes, fmtCount, fmtRate, onset, useFleetStats } from "../lib/stats";
import { freshnessText, RangeSelect, toRows, useRange } from "./StatsParts";

/** Fleet-wide rates and sessions from the aggregator's rolling history (`GET /fleet/stats`). A single instance is one row of the same data. */
export function OverviewPage() {
  const [range, setRange] = useRange();
  const q = useFleetStats(range);
  const stats = q.data;
  const f = stats && freshnessText(stats, stats.fleet.last_sample_ms);
  const points = stats?.fleet.points ?? [];

  return (
    <div className="max-w-4xl">
      <PageHeader
        title="Overview"
        summary={f ? <span className={f.stale ? "text-warn" : undefined}>{f.text}. Rates are per second, averaged over each minute.</span> : "Traffic across every instance."}
        actions={<RangeSelect value={range} onChange={setRange} />}
      />
      {q.isPending && <Loading rows={4} />}
      {q.error && <ErrorState onRetry={() => q.refetch()}>{(q.error as Error).message}. The aggregator may not be configured for this controller.</ErrorState>}
      {stats && stats.fleet.last_sample_ms === null && (
        <EmptyState>No instance has reported in this range. Charts appear after a proxy pushes its first stats.</EmptyState>
      )}
      {stats && stats.fleet.last_sample_ms !== null && (
        <>
          <TimeSeriesChart
            title="Connections"
            rows={toRows(points, (p) => ({ accepted: p.accepted_per_sec, blocked: p.blocked_per_sec }))}
            series={[
              { key: "accepted", label: "Accepted" },
              { key: "blocked", label: "Blocked" },
            ]}
            format={fmtRate}
            onsetAt={onset(points, (p) => p.blocked_per_sec, (p) => p.t_ms)}
            onsetLabel="Blocking"
          />
          <TimeSeriesChart title="Throughput" rows={toRows(points, (p) => ({ bytes: p.bytes_per_sec }))} series={[{ key: "bytes", label: "Bytes" }]} format={fmtBytes} />
          <TimeSeriesChart
            title="Sessions"
            rows={toRows(points, (p) => ({ tcp: p.tcp_sessions, udp: p.udp_sessions }))}
            series={[
              { key: "tcp", label: "TCP" },
              { key: "udp", label: "UDP" },
            ]}
            format={fmtCount}
          />
        </>
      )}
    </div>
  );
}
