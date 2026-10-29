import { StatusMark } from "../components/ui/StatusMark";
import { Link, useSearchParams } from "react-router-dom";
import { EmptyState, ErrorState, Loading } from "../components/shell/States";
import { PageHeader } from "../components/shell/Page";
import { TimeSeriesChart } from "../components/charts/TimeSeriesChart";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { fmtBytes, fmtRate, onset, useFleetStats } from "../lib/stats";
import { freshnessText, RangeSelect, toRows, useRange } from "./StatsParts";

export function ListenersPage() {
  const [range, setRange] = useRange();
  const [params] = useSearchParams();
  const q = useFleetStats(range);
  const stats = q.data;
  const listeners = stats?.listeners ?? [];
  const current = listeners.find((l) => l.name === params.get("name")) ?? listeners[0];
  const f = stats && freshnessText(stats, stats.fleet.last_sample_ms);
  const points = current?.points ?? [];

  return (
    <div className="max-w-4xl">
      <PageHeader
        title="Listeners"
        summary={f ? <span>{f.stale && <StatusMark state="degraded" word="" />} {f.text}. Summed over every instance that runs the listener.</span> : "Traffic per listener."}
        actions={<RangeSelect value={range} onChange={setRange} />}
      />
      {q.isPending && <Loading rows={4} />}
      {q.error && <ErrorState onRetry={() => q.refetch()}>{(q.error as Error).message}</ErrorState>}
      {stats && listeners.length === 0 && <EmptyState>No listener has reported traffic in this range.</EmptyState>}
      {current && (
        <>
          <div className="mb-6">
            <Table aria-label="Listeners, latest minute">
              <THead>
                <TR>
                  <TH>Listener</TH>
                  <TH numeric>Accepted</TH>
                  <TH numeric>Blocked</TH>
                  <TH numeric>Throughput</TH>
                </TR>
              </THead>
              <TBody>
                {listeners.map((l) => {
                  const p = l.points[l.points.length - 1];
                  return (
                    <TR key={l.name} selected={l === current}>
                      <TD className="font-mono">
                        <Link replace className="underline" aria-current={l === current || undefined} to={`?${new URLSearchParams({ range, name: l.name })}`}>
                          {l.name}
                        </Link>
                      </TD>
                      <TD numeric>{p ? fmtRate(p.accepted_per_sec) : "–"}</TD>
                      <TD numeric>{p ? fmtRate(p.blocked_per_sec) : "–"}</TD>
                      <TD numeric>{p ? fmtBytes(p.bytes_c2s_per_sec + p.bytes_s2c_per_sec) : "–"}</TD>
                    </TR>
                  );
                })}
              </TBody>
            </Table>
          </div>
          <h2 className="mb-2 font-mono text-lg font-semibold">{current.name}</h2>
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
          <TimeSeriesChart
            title="Throughput"
            rows={toRows(points, (p) => ({ c2s: p.bytes_c2s_per_sec, s2c: p.bytes_s2c_per_sec }))}
            series={[
              { key: "c2s", label: "Client to server" },
              { key: "s2c", label: "Server to client" },
            ]}
            format={fmtBytes}
          />
        </>
      )}
    </div>
  );
}
