/** Pure helpers of the listener and route editors. Shapes follow the config schema (`RawListener`, `RawRoute`, `RawMatch`, `RawAction`). */

export type Json = Record<string, any>;

/** The fields each match type takes, in the order the editor shows them; the config parser rejects any other. */
export const MATCH_TYPES: Record<string, { label: string; fields: ("cidrs" | "ports" | "host" | "prefix" | "length" | "sniffer")[]; help: string }> = {
  always: { label: "Everything", fields: [], help: "Matches every connection. Put it last as the fallback." },
  client_cidr: { label: "Client address", fields: ["cidrs"], help: "The client's source IP is in one of these prefixes." },
  dst: { label: "Destination address", fields: ["cidrs"], help: "The destination IP is in one of these prefixes." },
  port: { label: "Destination port", fields: ["ports"], help: "A port such as 30001 or a range such as 30010-30020." },
  first_bytes: { label: "First bytes", fields: ["prefix", "length"], help: "The first bytes start with the prefix and/or their length is in range." },
  sni: { label: "TLS server name", fields: ["host"], help: "The hostname in the TLS hello: exact, *.suffix or .suffix." },
  sniffer: { label: "Sniffer", fields: ["sniffer", "host"], help: "A sniffer module recognises the protocol; hosts are optional." },
};

export function uniqueName(base: string, taken: string[]): string {
  for (let i = 1; ; i++) if (!taken.includes(`${base}-${i}`)) return `${base}-${i}`;
}

export const newListener = (existing: { name?: string }[]): Json => ({
  name: uniqueName("listener", existing.map((l) => l.name ?? "")),
  bind: "0.0.0.0:25565",
  protocol: "tcp",
});

export const newRoute = (pools: string[]): Json => ({ match: { type: "always" }, action: pools[0] ? { pool: pools[0] } : {} });

/** Switch a match to another type, dropping the fields the new type does not take (the config would refuse them). */
export function changeMatchType(match: Json, type: string): Json {
  const keep = MATCH_TYPES[type]?.fields ?? [];
  return { type, ...Object.fromEntries(Object.entries(match).filter(([k]) => keep.includes(k as any))) };
}

export function moveRoute<T>(routes: T[], index: number, by: -1 | 1): T[] {
  const to = index + by;
  if (to < 0 || to >= routes.length) return routes;
  const next = [...routes];
  [next[index], next[to]] = [next[to], next[index]];
  return next;
}

export function routeTarget(route: Json): { kind: "pool" | "resolver"; name: string } | null {
  const a = route.action ?? {};
  if (a.pool) return { kind: "pool", name: a.pool };
  if (a.resolver) return { kind: "resolver", name: a.resolver };
  return null;
}

/** One route in words, for the diagram's label and the table equivalent. */
export function describeRoute(route: Json): string {
  const m = route.match ?? {};
  const list = (v: unknown) => (Array.isArray(v) ? v.join(", ") : "");
  const what: Record<string, string> = {
    always: "everything",
    client_cidr: `client ${list(m.cidrs)}`,
    dst: `destination ${list(m.cidrs)}`,
    port: `destination port ${list(m.ports)}`,
    first_bytes: `first bytes ${[m.prefix, m.length && `length ${m.length.min}-${m.length.max}`].filter(Boolean).join(" ")}`,
    sni: `SNI ${list(m.host)}`,
    sniffer: `sniffer ${m.sniffer ?? ""}${m.host?.length ? ` ${list(m.host)}` : ""}`,
  };
  const t = routeTarget(route);
  return `${(what[m.type] ?? `${m.type}`).trim()} to ${t ? `${t.kind} ${t.name}` : "nothing yet"}`;
}
