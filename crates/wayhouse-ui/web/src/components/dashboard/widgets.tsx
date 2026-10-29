import { useQuery } from "@tanstack/react-query";
import { createContext, useContext } from "react";
import { Link } from "react-router-dom";
import { listAcmeSites, listRevisions } from "../../api";
import { fmtBytes, fmtCount, fmtRate, onset, parseRange, RANGES, useFleetStats, type Range } from "../../lib/stats";
import { toRows } from "../../pages/StatsParts";
import { useFleetSocket } from "../../useFleetSocket";
import type { FleetInstanceView } from "../../types";
import { TimeSeriesChart } from "../charts/TimeSeriesChart";
import { StatusMark } from "../ui/StatusMark";
import { Async, Headline, Panel, Showing, updated } from "./WidgetFrame";

export interface WidgetProps {
  settings: Record<string, unknown>;
}
/** A setting the inspector offers for a widget. */
export type SettingDef = { key: "range"; label: string } | { key: "top"; label: string };
export interface WidgetDef {
  /** Stable and versioned: a stored dashboard names the widget by this. */
  id: string;
  title: string;
  defaultWidth: 4 | 6 | 12;
  widths: number[];
  settings: SettingDef[];
  Component: (p: WidgetProps) => React.JSX.Element;
}

const rangeOf = (s: Record<string, unknown>): Range => parseRange(typeof s.range === "string" ? s.range : null);
const topOf = (s: Record<string, unknown>) => (Number.isInteger(s.top) && (s.top as number) > 0 ? Math.min(s.top as number, 50) : 5);
const rangeLabel = (r: Range) => RANGES.find((x) => x.value === r)!.label;

interface Feed {
  instances: FleetInstanceView[];
  connected: boolean;
}
const FeedCtx = createContext<Feed>({ instances: [], connected: false });

/** One fleet socket for the whole dashboard: every fleet widget reads it, so a page holds a single connection and one copy of the payload. */
export function FleetFeed({ children }: { children: React.ReactNode }) {
  const feed = useFleetSocket();
  return <FeedCtx.Provider value={feed}>{children}</FeedCtx.Provider>;
}
const useFleet = () => useContext(FeedCtx);

function Nodes() {
  const { instances, connected } = useFleet();
  const stale = instances.filter((i) => i.stale).length;
  const up = instances.length - stale;
  return (
    <Panel title="Nodes" footer={connected ? "Live" : "Disconnected: showing the last known view"}>
      {instances.length === 0 ? (
        <p className="py-4 text-sm text-ink-muted">{connected ? "No instance has reported yet." : "Waiting for the fleet feed."}</p>
      ) : (
        <>
          <Headline value={fmtCount(instances.length)} unit={instances.length === 1 ? "node" : "nodes"} />
          <div role="img" aria-label={`${up} reporting, ${stale} stale`} className="flex h-2 overflow-hidden rounded-sm bg-line">
            <span className="bg-good" style={{ width: `${(up / instances.length) * 100}%` }} />
            <span className="bg-warn" style={{ width: `${(stale / instances.length) * 100}%` }} />
          </div>
          <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1">
            <StatusMark state="up" word={`${fmtCount(up)} reporting`} />
            {stale > 0 && <StatusMark state="degraded" word={`${fmtCount(stale)} stale`} />}
          </div>
        </>
      )}
    </Panel>
  );
}

interface Issue {
  key: string;
  state: "down" | "degraded";
  text: string;
}
/** Everything in the live fleet view that needs a person, worst first. */
export function issuesOf(instances: FleetInstanceView[]): Issue[] {
  const out: Issue[] = [];
  for (const i of instances) {
    if (i.stale) out.push({ key: `${i.instance}/stale`, state: "down", text: `${i.instance} has not reported for ${Math.round(i.last_seen_ms_ago / 1000)} s` });
    if (i.skew === "outside-window") out.push({ key: `${i.instance}/skew`, state: "degraded", text: `${i.instance} runs ${i.version || "an unknown version"}, outside the supported window` });
    for (const p of i.pools) for (const b of p.backends) if (!b.healthy && b.state === "enabled") out.push({ key: `${i.instance}/${p.name}/${b.addr}`, state: "down", text: `${b.addr} in ${p.name} is unhealthy on ${i.instance}` });
  }
  return out.sort((a, b) => (a.state === b.state ? 0 : a.state === "down" ? -1 : 1));
}

function Attention({ settings }: WidgetProps) {
  const { instances, connected } = useFleet();
  const top = topOf(settings);
  const all = issuesOf(instances);
  return (
    <Panel title="Needs attention" footer={connected ? "Live" : "Disconnected: showing the last known view"}>
      {all.length === 0 ? (
        <StatusMark state="up" word="Nothing needs attention" />
      ) : (
        <>
          <Headline value={fmtCount(all.length)} unit={all.length === 1 ? "issue" : "issues"} />
          <ul className="flex flex-col gap-1">
            {all.slice(0, top).map((x) => (
              <li key={x.key}>
                <StatusMark state={x.state} word={<span className="break-all">{x.text}</span>} />
              </li>
            ))}
          </ul>
          <Showing shown={Math.min(top, all.length)} total={all.length}>
            <Link className="underline" to="/fleet">
              Open the fleet
            </Link>
          </Showing>
        </>
      )}
    </Panel>
  );
}

function Connections({ settings }: WidgetProps) {
  const range = rangeOf(settings);
  const q = useFleetStats(range);
  return (
    <Panel title="Connections over time" footer={`${rangeLabel(range)}${q.dataUpdatedAt ? ` · ${updated(q.dataUpdatedAt)}` : ""}`}>
      <Async q={q} empty={(s) => (s.fleet.last_sample_ms === null ? "No data for this range: no instance has reported." : null)}>
        {(s) => {
          const pts = s.fleet.points;
          const last = pts[pts.length - 1];
          return (
            <>
              <Headline value={fmtRate(last?.accepted_per_sec ?? 0)} unit="accepted" />
              <TimeSeriesChart
                title="Connections"
                rows={toRows(pts, (p) => ({ accepted: p.accepted_per_sec, blocked: p.blocked_per_sec }))}
                series={[
                  { key: "accepted", label: "Accepted" },
                  { key: "blocked", label: "Blocked" },
                ]}
                format={fmtRate}
                onsetAt={onset(pts, (p) => p.blocked_per_sec, (p) => p.t_ms)}
                onsetLabel="Blocking"
                height={140}
              />
            </>
          );
        }}
      </Async>
    </Panel>
  );
}

function Throughput({ settings }: WidgetProps) {
  const range = rangeOf(settings);
  const top = topOf(settings);
  const q = useFleetStats(range);
  return (
    <Panel title="Throughput by region" footer={`${rangeLabel(range)}${q.dataUpdatedAt ? ` · ${updated(q.dataUpdatedAt)}` : ""}`}>
      <Async q={q} empty={(s) => (s.regions.length === 0 ? "No data for this range: no region has reported." : null)}>
        {(s) => {
          const rows = s.regions
            .map((r) => ({ name: r.name, value: r.points[r.points.length - 1]?.bytes_per_sec ?? 0 }))
            .sort((a, b) => b.value - a.value);
          const max = Math.max(...rows.map((r) => r.value), 1);
          const total = rows.reduce((n, r) => n + r.value, 0);
          return (
            <>
              <Headline value={fmtBytes(total)} unit="across regions" />
              <ul aria-label="Throughput by region" className="flex flex-col gap-1.5">
                {rows.slice(0, top).map((r) => (
                  <li key={r.name} className="grid grid-cols-[6rem_1fr_auto] items-center gap-2 text-sm">
                    <span className="truncate font-mono">{r.name}</span>
                    <span aria-hidden className="h-2 bg-line">
                      <span className="block h-2 bg-series-1" style={{ width: `${(r.value / max) * 100}%` }} />
                    </span>
                    <span className="tabular-nums">{fmtBytes(r.value)}</span>
                  </li>
                ))}
              </ul>
              <Showing shown={Math.min(top, rows.length)} total={rows.length} />
            </>
          );
        }}
      </Async>
    </Panel>
  );
}

function Rollout() {
  const { instances, connected } = useFleet();
  const q = useQuery({ queryKey: ["revisions"], queryFn: listRevisions, refetchInterval: 30_000 });
  const versions = new Map<string, number>();
  for (const i of instances) versions.set(i.version || "unknown", (versions.get(i.version || "unknown") ?? 0) + 1);
  return (
    <Panel title="Config rollout" footer={[q.dataUpdatedAt ? updated(q.dataUpdatedAt) : null, "Per-instance applied revisions are not reported yet"].filter(Boolean).join(" · ")}>
      <Async q={q} empty={(r) => (r.length === 0 ? "No configuration has been submitted yet." : null)}>
        {(revs) => {
          const current = revs.find((r) => r.current) ?? revs[0];
          return (
            <>
              <Headline value={`r${current.revision}`} unit={`current of ${revs.length} revisions`} />
              <p className="mb-1 text-xs text-ink-muted">Software versions across the fleet{connected ? "" : " (last known)"}</p>
              <ul className="flex flex-col gap-0.5 text-sm">
                {[...versions].sort((a, b) => b[1] - a[1]).map(([v, n]) => (
                  <li key={v} className="flex justify-between">
                    <span className="font-mono">{v}</span>
                    <span className="tabular-nums">{fmtCount(n)}</span>
                  </li>
                ))}
              </ul>
              <Showing shown={versions.size} total={versions.size}>
                <Link className="underline" to="/config-history">
                  Config history
                </Link>
              </Showing>
            </>
          );
        }}
      </Async>
    </Panel>
  );
}

function Certificates({ settings }: WidgetProps) {
  const top = topOf(settings);
  const q = useQuery({ queryKey: ["acme-sites"], queryFn: listAcmeSites, refetchInterval: 60_000 });
  return (
    <Panel title="Certificates" footer={q.dataUpdatedAt ? updated(q.dataUpdatedAt) : undefined}>
      <Async q={q} empty={(r) => (r.sites.some((s) => s.certificate) ? null : "No certificate has been issued yet.")}>
        {({ sites }) => {
          const now = Date.now() / 1000;
          const withCert = sites.filter((s) => s.certificate).sort((a, b) => a.certificate!.not_after - b.certificate!.not_after);
          const soon = withCert.filter((s) => s.certificate!.not_after - now < 14 * 86_400).length;
          return (
            <>
              <Headline value={fmtCount(withCert.length)} unit={soon ? `certificates, ${soon} expire within 14 days` : "certificates"} />
              <ul className="flex flex-col gap-1">
                {withCert.slice(0, top).map((s) => {
                  const days = Math.floor((s.certificate!.not_after - now) / 86_400);
                  return (
                    <li key={s.name} className="flex justify-between gap-2 text-sm">
                      <StatusMark state={days < 0 ? "down" : days < 14 ? "degraded" : "up"} word={<span className="font-mono">{s.name}</span>} />
                      <span className="tabular-nums">{days < 0 ? `expired ${-days} d ago` : `${days} d left`}</span>
                    </li>
                  );
                })}
              </ul>
              <Showing shown={Math.min(top, withCert.length)} total={withCert.length}>
                <Link className="underline" to="/certificates">
                  All certificates
                </Link>
              </Showing>
            </>
          );
        }}
      </Async>
    </Panel>
  );
}

export const WIDGETS: WidgetDef[] = [
  { id: "nodes.v1", title: "Nodes", defaultWidth: 6, widths: [4, 6, 12], settings: [], Component: Nodes },
  { id: "attention.v1", title: "Needs attention", defaultWidth: 6, widths: [4, 6, 12], settings: [{ key: "top", label: "Rows shown" }], Component: Attention },
  { id: "connections.v1", title: "Connections over time", defaultWidth: 6, widths: [6, 12], settings: [{ key: "range", label: "Time range" }], Component: Connections },
  { id: "throughput.v1", title: "Throughput by region", defaultWidth: 6, widths: [4, 6, 12], settings: [{ key: "range", label: "Time range" }, { key: "top", label: "Rows shown" }], Component: Throughput },
  { id: "rollout.v1", title: "Config rollout", defaultWidth: 6, widths: [4, 6, 12], settings: [], Component: Rollout },
  { id: "certificates.v1", title: "Certificates", defaultWidth: 6, widths: [4, 6, 12], settings: [{ key: "top", label: "Rows shown" }], Component: Certificates },
];
export const widgetById = (id: string) => WIDGETS.find((w) => w.id === id);
export { rangeOf, topOf };
