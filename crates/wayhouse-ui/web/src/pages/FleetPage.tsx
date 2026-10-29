import { useEffect, useMemo, useRef, useState } from "react";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useSearchParams } from "react-router-dom";
import type { ColumnDef } from "@tanstack/react-table";
import { ApiError, addBackend, drainInstance, getFleetNodes, resolveFleetNodes, routeHint, undrainInstance } from "../api";
import { useFleetSocket } from "../useFleetSocket";
import {
  NODE_STATES,
  SORT_KEYS,
  filterOf,
  filterQuery,
  nodeQuery,
  parseNodeQuery,
  savedFilter,
  toSavedFilter,
  type Filter,
  type FleetNode,
  type NodeQuery,
  type Rollup,
} from "../lib/fleet/nodes";
import { useDeleteFilter, useFilters, useSaveFilter } from "../lib/uidata";
import { useCan } from "../lib/session";
import { NodeInspector } from "../components/fleet/NodeInspector";
import { NodeStateBadge } from "../components/fleet/NodeStateBadge";
import { PageHeader } from "../components/shell/Page";
import { useInspector } from "../components/shell/Inspector";
import { EmptyState, ErrorState, Loading } from "../components/shell/States";
import { Badge } from "../components/ui/Badge";
import { Button, Input } from "../components/ui/Button";
import { DataTable, type Group } from "../components/ui/DataTable";
import { Select } from "../components/ui/Input";
import { Pager } from "../components/ui/Pager";
import { StatusMark } from "../components/ui/StatusMark";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { useToast } from "../components/ui/Toast";

const POLL_MS = 5000;
const SHOWN_TAGS = 3;

const rollupText = (r: Rollup) =>
  [`${r.nodes} node${r.nodes === 1 ? "" : "s"}`, r.degraded && `${r.degraded} degraded`, r.stale && `${r.stale} stale`, `${r.sessions} sessions`].filter(Boolean).join(" · ");

const hasFilter = (f: Filter) => !!(f.q || f.state || f.region || f.tags.length);

function columns(): ColumnDef<FleetNode>[] {
  return [
    { id: "instance", header: "Node", enableSorting: false, cell: ({ row }) => <span className="font-mono">{row.original.instance}</span> },
    { id: "state", header: "State", enableSorting: false, cell: ({ row }) => <NodeStateBadge state={row.original.state} /> },
    {
      id: "version",
      header: "Version",
      enableSorting: false,
      cell: ({ row: { original: n } }) => (
        <span className="inline-flex items-center gap-2">
          <span className="font-mono text-xs">{n.version ? `v${n.version}` : "?"}</span>
          {n.skew === "within-window" && <Badge tone="warn">older, in window</Badge>}
          {n.skew === "outside-window" && <Badge tone="bad">outside window</Badge>}
        </span>
      ),
    },
    {
      id: "tags",
      header: "Tags",
      enableSorting: false,
      cell: ({ row }) => {
        const tags = Object.entries(row.original.tags);
        return (
          <span className="font-mono text-xs text-ink-muted">
            {tags.slice(0, SHOWN_TAGS).map(([k, v]) => `${k}=${v}`).join(" ")}
            {tags.length > SHOWN_TAGS && ` +${tags.length - SHOWN_TAGS}`}
          </span>
        );
      },
    },
    { id: "sessions", header: "Sessions", enableSorting: false, meta: { numeric: true }, cell: ({ row }) => row.original.sessions.tcp + row.original.sessions.udp },
    { id: "seen", header: "Last seen", enableSorting: false, meta: { numeric: true }, cell: ({ row }) => `${(row.original.last_seen_ms_ago / 1000).toFixed(0)} s ago` },
  ];
}

export function FleetPage() {
  const [params, setParams] = useSearchParams();
  const q = useMemo(() => parseNodeQuery(params), [params]);
  const canOperate = useCan("operator");
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const inspector = useInspector();
  const { instances, connected } = useFleetSocket();

  /** Change the URL state. A filter change goes back to page 1; the stable setter keeps inspector callbacks current. */
  const update = (patch: Partial<NodeQuery>) =>
    setParams(
      (prev) => {
        const cur = parseNodeQuery(prev);
        const next = { ...cur, ...patch };
        const filterChanged = filterQuery(cur) !== filterQuery(next);
        if (filterChanged && patch.page === undefined) next.page = 1;
        return new URLSearchParams(nodeQuery(next));
      },
      { replace: true },
    );

  const apiQuery = nodeQuery({ ...q, sel: "" });
  const nodes = useQuery({
    queryKey: ["fleet", "nodes", apiQuery],
    queryFn: () => getFleetNodes(apiQuery),
    refetchInterval: POLL_MS,
    placeholderData: keepPreviousData,
  });
  const data = nodes.data;

  // The filter shrank under the page the URL asks for.
  useEffect(() => {
    if (data && data.items.length === 0 && data.total > 0 && q.page > 1) update({ page: 1 });
  });

  // Search text: typed freely, written to the URL after a short pause.
  const [text, setText] = useState(q.q);
  useEffect(() => setText(q.q), [q.q]);
  useEffect(() => {
    if (text === q.q) return;
    const t = setTimeout(() => update({ q: text }), 250);
    return () => clearTimeout(t);
  }, [text]); // eslint-disable-line react-hooks/exhaustive-deps

  // Inspector follows `sel`. It holds a snapshot, so it is re-opened as the row or the live view changes.
  const node = data?.items.find((n) => n.instance === q.sel);
  const live = instances.find((i) => i.instance === q.sel);
  const shown = useRef(false);
  const addTag = (tag: string) => update({ tags: [...new Set([...parseNodeQuery(new URLSearchParams(window.location.search)).tags, tag])] });
  useEffect(() => {
    if (shown.current && !inspector.current && q.sel) {
      shown.current = false;
      update({ sel: "" });
    }
  }, [inspector.current]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!q.sel) {
      if (shown.current) inspector.close();
      shown.current = false;
      return;
    }
    shown.current = true;
    inspector.open(q.sel, <NodeInspector instance={q.sel} node={node} live={live} onTag={addTag} />);
  }, [q.sel, node, live]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => () => inspector.close(), []); // eslint-disable-line react-hooks/exhaustive-deps

  const groups: Group<FleetNode>[] = useMemo(() => {
    const rollups = new Map((data?.groups ?? []).map((g) => [g.region, g]));
    const out: Group<FleetNode>[] = [];
    for (const n of data?.items ?? []) {
      const key = n.region ?? "";
      let g = out.find((x) => x.key === key);
      if (!g) {
        const r = rollups.get(n.region ?? null);
        g = { key, label: n.region ?? "No region", rollup: r && rollupText(r), rows: [] };
        out.push(g);
      }
      g.rows.push(n);
    }
    return out;
  }, [data]);
  const cols = useMemo(columns, []);

  async function bulk(kind: "drain" | "undrain") {
    let found;
    try {
      found = await resolveFleetNodes(filterQuery(q));
    } catch (e) {
      toast(`${kind} failed: ${e instanceof ApiError ? e.message : e}`, "down");
      return;
    }
    if (found.count === 0) return toast("No nodes match this filter");
    const s = found.count === 1 ? "" : "s";
    const ok = await confirm({
      title: `${kind === "drain" ? "Drain" : "Undrain"} ${found.count} node${s}?`,
      description: `${found.count} node${s} match the current filter${hasFilter(q) ? "" : " (the whole fleet)"}. ${
        kind === "drain" ? "Each stops accepting new sessions; existing ones finish." : "Each starts accepting new sessions again."
      }`,
      confirmLabel: `${kind === "drain" ? "Drain" : "Undrain"} ${found.count}`,
      danger: kind === "drain",
    });
    if (!ok) return;
    const act = kind === "drain" ? drainInstance : undrainInstance;
    const results = await Promise.allSettled(found.instances.map((i) => act(i)));
    const failed = results.filter((r) => r.status === "rejected").length;
    if (failed) toast(`${kind}: ${found.count - failed} of ${found.count} done, ${failed} failed`, "down");
    else toast(`${kind}: ${found.count} node${s} done`);
  }

  return (
    <div>
      <PageHeader
        title="Fleet"
        summary="Every node that reports to the aggregator, by region. Filter, then act on one node or on everything the filter selects."
        actions={<StatusMark state={connected ? "up" : "down"} word={connected ? "live" : "reconnecting…"} />}
      />

      <FilterBar q={q} text={text} onText={setText} update={update} />

      {nodes.isPending ? (
        <Loading rows={8} />
      ) : nodes.isError && !data ? (
        <ErrorState onRetry={() => nodes.refetch()}>Could not load the node list: {nodes.error instanceof ApiError ? nodes.error.message : String(nodes.error)}. Check that the aggregator is reachable.</ErrorState>
      ) : data && data.rollup.nodes === 0 ? (
        hasFilter(q) ? (
          <EmptyState action={<Button onClick={() => update({ q: "", state: "", region: "", tags: [] })}>Clear filters</Button>}>No nodes match this filter.</EmptyState>
        ) : (
          <EmptyState>No instances have pushed a fleet-state summary yet.</EmptyState>
        )
      ) : data ? (
        <>
          <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
            <p role="status" className="text-sm text-ink-muted tabular-nums">
              {hasFilter(q) ? "Matching: " : "Fleet: "}
              {rollupText(data.rollup)}
            </p>
            {canOperate && (
              <span className="flex gap-2">
                <Button onClick={() => bulk("drain")}>Drain all matching…</Button>
                <Button onClick={() => bulk("undrain")}>Undrain all matching…</Button>
              </span>
            )}
          </div>
          <DataTable columns={cols} data={data.items} groups={groups} getRowId={(n) => n.instance} selectedId={q.sel || null} onOpen={(n) => update({ sel: n.instance })} />
          <Pager page={data.page} size={data.size} total={data.total} onPage={(page) => update({ page })} onSize={(size) => update({ size, page: 1 })} />
        </>
      ) : null}

      {!canOperate && <p className="mt-2 text-sm text-ink-muted">Your role can view the fleet but not change it.</p>}
      <BroadcastForms disabled={!canOperate} />
      {dialog}
    </div>
  );
}

function FilterBar({ q, text, onText, update }: { q: NodeQuery; text: string; onText: (s: string) => void; update: (p: Partial<NodeQuery>) => void }) {
  const [tagDraft, setTagDraft] = useState("");
  return (
    <section aria-label="Filters" className="mb-3 flex flex-col gap-2">
      <div className="flex flex-wrap items-end gap-2">
        <label className="flex flex-col gap-0.5 text-xs text-ink-muted">
          Search
          <Input value={text} onChange={(e) => onText(e.target.value)} placeholder="node, region, version, tag" className="w-56" />
        </label>
        <label className="flex flex-col gap-0.5 text-xs text-ink-muted">
          State
          <Select aria-label="State" value={q.state} onChange={(e) => update({ state: e.target.value as NodeQuery["state"] })}>
            <option value="">any</option>
            {NODE_STATES.map((s) => (
              <option key={s}>{s}</option>
            ))}
          </Select>
        </label>
        <label className="flex flex-col gap-0.5 text-xs text-ink-muted">
          Region
          <Input value={q.region} onChange={(e) => update({ region: e.target.value })} placeholder="any" className="w-32" />
        </label>
        <form
          className="flex items-end gap-1"
          onSubmit={(e) => {
            e.preventDefault();
            const t = tagDraft.trim();
            if (t) update({ tags: [...new Set([...q.tags, t])] });
            setTagDraft("");
          }}
        >
          <label className="flex flex-col gap-0.5 text-xs text-ink-muted">
            Tag
            <Input value={tagDraft} onChange={(e) => setTagDraft(e.target.value)} placeholder="key or key=value" className="w-40" mono />
          </label>
          <Button type="submit">Add tag</Button>
        </form>
        <label className="flex flex-col gap-0.5 text-xs text-ink-muted">
          Sort by
          <span className="flex gap-1">
            <Select aria-label="Sort by" value={q.sort} onChange={(e) => update({ sort: e.target.value as NodeQuery["sort"] })}>
              {SORT_KEYS.map((k) => (
                <option key={k}>{k}</option>
              ))}
            </Select>
            <Button aria-label={q.dir === "asc" ? "Ascending, switch to descending" : "Descending, switch to ascending"} onClick={() => update({ dir: q.dir === "asc" ? "desc" : "asc" })}>
              {q.dir === "asc" ? "↑" : "↓"}
            </Button>
          </span>
        </label>
      </div>
      {q.tags.length > 0 && (
        <ul aria-label="Tag filters" className="flex flex-wrap gap-1">
          {q.tags.map((t) => (
            <li key={t}>
              <button type="button" aria-label={`Remove tag filter ${t}`} onClick={() => update({ tags: q.tags.filter((x) => x !== t) })} className="rounded-sm border border-line px-1.5 py-0.5 font-mono text-xs hover:bg-surface-sunken">
                {t} ×
              </button>
            </li>
          ))}
        </ul>
      )}
      <SavedFilters q={q} update={update} />
    </section>
  );
}

function SavedFilters({ q, update }: { q: NodeQuery; update: (p: Partial<NodeQuery>) => void }) {
  const canWrite = useCan("operator");
  const saved = useFilters();
  const save = useSaveFilter();
  const remove = useDeleteFilter();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const [active, setActive] = useState("");
  const [naming, setNaming] = useState<string | null>(null);
  const names = (saved.data ?? []).map((f) => f.name);
  if (!saved.data?.length && !canWrite) return null;

  const fail = (what: string) => (e: unknown) => toast(`${what} failed: ${e instanceof ApiError ? e.message : e}`, "down");
  return (
    <div className="flex flex-wrap items-center gap-2">
      <label className="flex items-center gap-1.5 text-sm text-ink-muted">
        Saved filters
        <Select
          value={active}
          onChange={(e) => {
            const f = (saved.data ?? []).find((x) => x.name === e.target.value);
            setActive(e.target.value);
            if (f) {
              const { q, state, region, tags } = savedFilter(f);
              update({ q, state, region, tags });
            }
          }}
        >
          <option value="">none applied</option>
          {names.map((n) => (
            <option key={n}>{n}</option>
          ))}
        </Select>
      </label>
      {canWrite && naming === null && (
        <Button disabled={!hasFilter(q)} onClick={() => setNaming(active)}>
          Save current filter…
        </Button>
      )}
      {canWrite && naming !== null && (
        <form
          className="flex items-center gap-1"
          onSubmit={(e) => {
            e.preventDefault();
            const name = naming.trim();
            if (!name) return;
            save
              .mutateAsync(toSavedFilter(name, filterOf(q)))
              .then(() => {
                setActive(name);
                setNaming(null);
                toast(`Saved filter ${name}`);
              })
              .catch(fail("save filter"));
          }}
        >
          <Input autoFocus aria-label="Filter name" value={naming} onChange={(e) => setNaming(e.target.value)} maxLength={64} className="w-40" />
          <Button type="submit" variant="primary">
            Save
          </Button>
          <Button onClick={() => setNaming(null)}>Cancel</Button>
        </form>
      )}
      {canWrite && active && naming === null && (
        <Button
          variant="danger"
          onClick={async () => {
            if (!(await confirm({ title: `Delete saved filter ${active}?`, description: "It disappears for every operator. The nodes are not affected.", confirmLabel: "Delete" }))) return;
            remove.mutateAsync(active).then(() => (setActive(""), toast(`Deleted filter ${active}`))).catch(fail("delete filter"));
          }}
        >
          Delete filter
        </Button>
      )}
      {dialog}
    </div>
  );
}

function BroadcastForms({ disabled }: { disabled: boolean }) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const [addPool, setAddPool] = useState("");
  const [addAddr, setAddAddr] = useState("");
  const [hintIp, setHintIp] = useState("");
  const [hintPool, setHintPool] = useState("");
  const [hintTtl, setHintTtl] = useState(30);

  function report(label: string, p: Promise<{ results: { instance: string; status: number | null; error: string | null }[] }>) {
    setBusy(true);
    p.then((r) => {
      const bad = r.results.filter((x) => x.error || (x.status ?? 0) >= 400).length;
      if (bad) toast(`${label}: ${bad} of ${r.results.length} instances failed`, "down");
      else toast(`${label}: ${r.results.length} instance${r.results.length === 1 ? "" : "s"} ok`);
    })
      .catch((e) => toast(`${label} failed: ${e instanceof ApiError ? e.message : e}`, "down"))
      .finally(() => setBusy(false));
  }
  const off = disabled || busy;

  return (
    <div className="mt-6 grid gap-4 min-[760px]:grid-cols-2">
      <section className="border border-line bg-surface p-4">
        <h2 className="mb-3 text-lg font-semibold">Add a backend (every instance)</h2>
        <form
          className="flex flex-wrap gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            report(`add ${addPool}/${addAddr}`, addBackend(addPool, addAddr));
          }}
        >
          <Input aria-label="Pool" placeholder="pool" value={addPool} onChange={(e) => setAddPool(e.target.value)} />
          <Input aria-label="Address" placeholder="ip:port" value={addAddr} onChange={(e) => setAddAddr(e.target.value)} />
          <Button type="submit" disabled={off}>
            Add
          </Button>
        </form>
      </section>

      <section className="border border-line bg-surface p-4">
        <h2 className="mb-3 text-lg font-semibold">Route hint (every instance)</h2>
        <form
          className="flex flex-wrap gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            report("route-hint", routeHint(hintIp, hintPool, hintTtl));
          }}
        >
          <Input aria-label="Source IP" placeholder="src ip" value={hintIp} onChange={(e) => setHintIp(e.target.value)} />
          <Input aria-label="Pool" placeholder="pool" value={hintPool} onChange={(e) => setHintPool(e.target.value)} />
          <Input aria-label="TTL in seconds" type="number" placeholder="ttl sec" value={hintTtl} onChange={(e) => setHintTtl(Number(e.target.value))} className="w-24" />
          <Button type="submit" disabled={off}>
            Set hint
          </Button>
        </form>
      </section>
    </div>
  );
}
