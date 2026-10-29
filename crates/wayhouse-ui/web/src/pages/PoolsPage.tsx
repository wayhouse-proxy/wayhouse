import { Link, useSearchParams } from "react-router-dom";
import { EmptyState, ErrorState, Loading } from "../components/shell/States";
import { PageHeader } from "../components/shell/Page";
import { TimeSeriesChart } from "../components/charts/TimeSeriesChart";
import { StatusMark, type State } from "../components/ui/StatusMark";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { fmtCount, useFleetStats } from "../lib/stats";
import { freshnessText, RangeSelect, toRows, useRange } from "./StatsParts";

/** All backends healthy is up, none is down, anything between is degraded. */
const stateOf = (healthy: number, total: number): State => (total === 0 ? "unknown" : healthy === total ? "up" : healthy === 0 ? "down" : "degraded");

export function PoolsPage() {
  const [range, setRange] = useRange();
  const [params] = useSearchParams();
  const q = useFleetStats(range);
  const stats = q.data;
  const pools = stats?.pools ?? [];
  const current = pools.find((p) => p.name === params.get("name")) ?? pools[0];
  const f = stats && freshnessText(stats, stats.fleet.last_sample_ms);

  return (
    <div className="max-w-4xl">
      <PageHeader
        title="Pools"
        summary={f ? <span>{f.stale && <StatusMark state="degraded" word="" />} {f.text}. Backend health as the latest instance reports it.</span> : "Backend health per pool."}
        actions={<RangeSelect value={range} onChange={setRange} />}
      />
      {q.isPending && <Loading rows={4} />}
      {q.error && <ErrorState onRetry={() => q.refetch()}>{(q.error as Error).message}</ErrorState>}
      {stats && pools.length === 0 && <EmptyState>No pool has reported in this range.</EmptyState>}
      {current && (
        <>
          <div className="mb-6">
            <Table aria-label="Pools, latest minute">
              <THead>
                <TR>
                  <TH>Pool</TH>
                  <TH>Health</TH>
                  <TH numeric>Healthy</TH>
                  <TH numeric>Backends</TH>
                </TR>
              </THead>
              <TBody>
                {pools.map((p) => {
                  const last = p.points[p.points.length - 1];
                  return (
                    <TR key={p.name} selected={p === current}>
                      <TD className="font-mono">
                        <Link replace className="underline" aria-current={p === current || undefined} to={`?${new URLSearchParams({ range, name: p.name })}`}>
                          {p.name}
                        </Link>
                      </TD>
                      <TD>{last ? <StatusMark state={stateOf(last.healthy_backends, last.backends)} /> : <StatusMark state="unknown" />}</TD>
                      <TD numeric>{last ? fmtCount(last.healthy_backends) : "–"}</TD>
                      <TD numeric>{last ? fmtCount(last.backends) : "–"}</TD>
                    </TR>
                  );
                })}
              </TBody>
            </Table>
          </div>
          <h2 className="mb-2 font-mono text-lg font-semibold">{current.name}</h2>
          <TimeSeriesChart
            title="Backends"
            rows={toRows(current.points, (p) => ({ healthy: p.healthy_backends, total: p.backends }))}
            series={[
              { key: "healthy", label: "Healthy" },
              { key: "total", label: "Configured" },
            ]}
            format={fmtCount}
          />
        </>
      )}
    </div>
  );
}
