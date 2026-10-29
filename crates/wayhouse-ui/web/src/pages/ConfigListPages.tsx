import { useMemo } from "react";
import { PoolLines, type BackendRow } from "../components/editors/PoolLines";
import { Select } from "../components/ui/Input";
import type { State } from "../components/ui/StatusMark";
import type { Json } from "../lib/editors/listeners";
import { SOURCE_TYPES, newPool, newResolver, newSource, poolFields, setPoolSource, sourceFields } from "../lib/editors/lists";
import { useFleetSocket } from "../useFleetSocket";
import { ConfigListPage } from "./ConfigListPage";

const count = (v: unknown) => (Array.isArray(v) ? v.length : 0);
const dash = (v: unknown) => (v === undefined || v === null || v === "" ? "–" : String(v));

/** Configure > Pools: the backends of each pool, as patch lines with live state. */
export function PoolsEditorPage() {
  const { instances } = useFleetSocket();
  /** Per pool and address: the health every reporting instance gives it. */
  const live = useMemo(() => {
    const m = new Map<string, Map<string, boolean[]>>();
    for (const inst of instances)
      for (const pool of inst.pools) {
        const backends = m.get(pool.name) ?? new Map<string, boolean[]>();
        for (const b of pool.backends) backends.set(b.addr, [...(backends.get(b.addr) ?? []), b.healthy]);
        m.set(pool.name, backends);
      }
    return m;
  }, [instances]);
  const rowsOf = (pool: Json): BackendRow[] => {
    const seen = live.get(String(pool.name)) ?? new Map<string, boolean[]>();
    const addrs = [...new Set([...(Array.isArray(pool.targets) ? pool.targets.map(String) : []), ...seen.keys()])];
    return addrs.map((addr) => {
      const h = seen.get(addr);
      const state: State = !h?.length ? "unknown" : h.every(Boolean) ? "up" : h.some(Boolean) ? "degraded" : "down";
      return { addr, state };
    });
  };

  return (
    <ConfigListPage
      listKey="pools"
      noun="pool"
      title="Pools"
      summary="The backends a listener route sends connections to, and how one is picked."
      page="pool"
      refKind="pool"
      columns={[
        { label: "Balancer", cell: (p) => dash(p.balancer ?? "round_robin") },
        { label: "Backends", cell: (p) => (p.source ? `from source ${p.source}` : count(p.targets)), numeric: true },
      ]}
      create={newPool}
      fieldsOf={poolFields}
      above={({ item, root, update, canWrite }) => {
        const sources: string[] = (Array.isArray(root.backend_sources) ? root.backend_sources : []).map((s: Json) => s.name).filter(Boolean);
        return (
          <>
            <PoolLines name={String(item.name)} source={item.source} rows={rowsOf(item)} />
            <label className="mb-3 flex max-w-md flex-col gap-1 text-sm">
              <span className="font-semibold">Backends come from</span>
              <Select aria-label="Backends come from" value={item.source ?? ""} disabled={!canWrite} onChange={(e) => update(setPoolSource(item, e.target.value))}>
                <option value="">The list below</option>
                {sources.map((s) => (
                  <option key={s} value={s}>{`Source ${s}`}</option>
                ))}
                {item.source && !sources.includes(item.source) && <option value={item.source}>{`${item.source} (not defined)`}</option>}
              </Select>
              <span className="text-ink-muted">A pool uses its own list or one backend source, not both.</span>
            </label>
          </>
        );
      }}
    />
  );
}

/** Configure > Backend sources: where a pool finds its backends when it is not a fixed list. */
export function SourcesEditorPage() {
  return (
    <ConfigListPage
      listKey="backend_sources"
      noun="source"
      title="Backend sources"
      summary="A named way to discover backends: a fixed list, DNS SRV, Consul, Kubernetes or a tunnel agent."
      page="source"
      refKind="source"
      columns={[
        { label: "Type", cell: (s) => SOURCE_TYPES.find((t) => t.value === s.type)?.label ?? dash(s.type) },
        { label: "Refresh", cell: (s) => (s.type === "static" ? "–" : `${s.refresh_interval_sec ?? 15} s`), numeric: true },
      ]}
      create={newSource}
      fieldsOf={(s) => sourceFields(String(s.type ?? "static"))}
      above={({ item, update, canWrite }) => (
        <label className="mb-3 flex max-w-md flex-col gap-1 text-sm">
          <span className="font-semibold">Type</span>
          <Select
            aria-label="Type"
            value={item.type ?? "static"}
            disabled={!canWrite}
            onChange={(e) => {
              // Fields of the old type would be refused by the config parser; keep only the common ones.
              const keep = new Set(sourceFields(e.target.value));
              update({ ...Object.fromEntries(Object.entries(item).filter(([k]) => keep.has(k))), type: e.target.value, ...(e.target.value === "static" && !item.targets ? { targets: [] } : {}) });
            }}
          >
            {SOURCE_TYPES.map((t) => (
              <option key={t.value} value={t.value}>
                {t.label}
              </option>
            ))}
          </Select>
          <span className="text-ink-muted">{SOURCE_TYPES.find((t) => t.value === (item.type ?? "static"))?.help}</span>
        </label>
      )}
    />
  );
}

/** Configure > Resolvers: an external service that decides which pool a connection goes to. */
export function ResolversEditorPage() {
  return (
    <ConfigListPage
      listKey="resolvers"
      noun="resolver"
      title="Resolvers"
      summary="An HTTP service the proxy asks which pool a connection goes to; a route can send traffic to one."
      page="resolver"
      refKind="resolver"
      columns={[
        { label: "Endpoint", cell: (r) => <span className="font-mono">{dash(r.endpoint)}</span> },
        { label: "On error", cell: (r) => dash(r.on_error ?? "reject") },
      ]}
      create={newResolver}
    />
  );
}
