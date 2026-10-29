import { uniqueName, type Json } from "./listeners";

/** Pure helpers of the pool, backend source and resolver editors. Shapes follow `RawPool`, `RawBackendSource`, `RawResolver`. */

export const newPool = (existing: { name?: string }[]): Json => ({ name: uniqueName("pool", existing.map((p) => p.name ?? "")), targets: [] });
export const newSource = (existing: { name?: string }[]): Json => ({ name: uniqueName("source", existing.map((s) => s.name ?? "")), type: "static", targets: [] });
export const newResolver = (existing: { name?: string }[]): Json => ({ name: uniqueName("resolver", existing.map((r) => r.name ?? "")), type: "http", endpoint: "" });

export const SOURCE_TYPES = [
  { value: "static", label: "Static list", help: "A fixed list of addresses." },
  { value: "dns_srv", label: "DNS SRV record", help: "Backends from a DNS SRV record, refreshed on an interval." },
  { value: "consul", label: "Consul service", help: "Healthy instances of a Consul service." },
  { value: "kubernetes", label: "Kubernetes service", help: "Endpoints of a Kubernetes service." },
  { value: "tunnel", label: "Tunnel agent", help: "A backend behind a WireGuard tunnel, identified by its public key." },
] as const;

const SOURCE_FIELDS: Record<string, string[]> = {
  static: ["targets"],
  dns_srv: ["record"],
  consul: ["service", "tag", "consul_addr", "consul_token_file"],
  kubernetes: ["service", "namespace", "port_name", "api"],
  tunnel: ["pubkey"],
};

/** The fields of a backend source the form shows for its type; the config parser rejects the others' meaning. */
export const sourceFields = (type: string): string[] => ["name", "refresh_interval_sec", ...(SOURCE_FIELDS[type] ?? [])];

/** The pool fields that apply: weights only to the weighted balancer, hash_on only to consistent hash, the list only without a source. */
export function poolFields(pool: Json): string[] {
  const all = ["name", "targets", "balancer", "hash_on", "weights", "health_check", "connect_timeout_ms", "idle_timeout_sec", "per_backend", "proxy_protocol"];
  return all.filter((f) => !(f === "weights" && pool.balancer !== "weighted") && !(f === "hash_on" && pool.balancer !== "consistent_hash") && !(f === "targets" && pool.source));
}

/** A pool takes its backends from the static list or from one source, never both. */
export function setPoolSource(pool: Json, source: string): Json {
  const { source: _s, targets: _t, ...rest } = pool;
  return source ? { ...rest, source } : { ...rest, targets: [] };
}

/** Where a pool, resolver or backend source is referred to, in words, so a removal can say what it leaves dangling. */
export function usedBy(config: Json, kind: "pool" | "resolver" | "source", name: string): string[] {
  const out: string[] = [];
  if (kind === "source") {
    for (const p of config.pools ?? []) if (p.source === name) out.push(`pool ${p.name}`);
    return out;
  }
  for (const l of config.listeners ?? []) {
    if (kind === "pool" && l.pool === name) out.push(`listener ${l.name}`);
    (l.routes ?? []).forEach((r: Json, i: number) => {
      if (r.action?.[kind] === name) out.push(`listener ${l.name} route ${i + 1}`);
    });
  }
  return out;
}

/** Drops what the chosen balancer does not take: the form hides `weights` and `hash_on`, but the YAML would keep them and the config parser refuses them. */
export function tidyPool(pool: Json): Json {
  const { weights, hash_on, ...rest } = pool;
  return {
    ...rest,
    ...(pool.balancer === "weighted" && weights !== undefined ? { weights } : {}),
    ...(pool.balancer === "consistent_hash" && hash_on !== undefined ? { hash_on } : {}),
  };
}
