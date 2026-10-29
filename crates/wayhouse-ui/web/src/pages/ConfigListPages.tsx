import { useMemo } from "react";
import { PoolLines, type BackendRow } from "../components/editors/PoolLines";
import { Select } from "../components/ui/Input";
import type { State } from "../components/ui/StatusMark";
import type { Json } from "../lib/editors/listeners";
import { SchemaForm } from "../components/form/SchemaForm";
import { Input } from "../components/ui/Input";
import { SOURCE_TYPES, newPool, newSite, setUpstreamTls, siteLabel, upstreamTls, newResolver, newSource, poolFields, setPoolSource, sourceFields, tidyPool } from "../lib/editors/lists";
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
      listPath={["pools"]}
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
      tidy={tidyPool}
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
      listPath={["backend_sources"]}
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
      listPath={["resolvers"]}
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

const TLS_MODES = [
  { value: "none", label: "Plain HTTP" },
  { value: "verify", label: "HTTPS, verify the certificate" },
  { value: "skip", label: "HTTPS, do not verify (private certificate)" },
  { value: "ca", label: "HTTPS, verify against a CA file" },
] as const;

/** Configure > Web sites: the sites the packaged Caddy serves (`web:`), and the gateway's own settings under them. */
export function WebEditorPage() {
  return (
    <ConfigListPage
      listPath={["web", "sites"]}
      noun="site"
      title="Web sites"
      summary="Hostnames the packaged Caddy serves, and the upstreams their requests go to."
      page="site"
      labelOf={siteLabel}
      columns={[
        { label: "Upstreams", cell: (s) => count(s.upstreams), numeric: true },
        { label: "Certificate", cell: (s) => (s.tls ? "files" : "from the controller") },
      ]}
      create={newSite}
      fieldsOf={() => ["hosts", "upstreams", "tls", "health_path"]}
      above={({ item, update, canWrite }) => {
        const { mode, caFile } = upstreamTls(item);
        return (
          <div className="mb-3 flex max-w-md flex-col gap-2 text-sm">
            <label className="flex flex-col gap-1">
              <span className="font-semibold">How Caddy talks to the upstreams</span>
              <Select aria-label="Upstream TLS" value={mode} disabled={!canWrite} onChange={(e) => update(setUpstreamTls(item, e.target.value as (typeof TLS_MODES)[number]["value"], caFile))}>
                {TLS_MODES.map((m) => (
                  <option key={m.value} value={m.value}>
                    {m.label}
                  </option>
                ))}
              </Select>
            </label>
            {mode === "ca" && (
              <label className="flex flex-col gap-1">
                <span className="font-semibold">CA file (a path on the Caddy side)</span>
                <Input mono aria-label="CA file" value={caFile} disabled={!canWrite} onChange={(e) => update(setUpstreamTls(item, "ca", e.target.value))} />
              </label>
            )}
          </div>
        );
      }}
      below={({ doc, layout, advanced, canWrite }) => (
        <section aria-label="Web gateway" className="mt-6">
          <h2 className="mb-1 text-lg font-semibold">Web gateway</h2>
          <p className="mb-2 max-w-(--measure) text-sm text-ink-muted">Where Caddy listens and which pool carries it; these apply to every site.</p>
          <SchemaForm schema={doc.schema!} value={doc.value} saved={doc.savedValue} onChange={doc.setValue} layout={layout} path={["web"]} page="web" only={["listen", "pool", "trusted_proxy", "internal_bind", "internal_ports"]} showAdvanced={advanced} readonly={!canWrite} />
        </section>
      )}
    />
  );
}
