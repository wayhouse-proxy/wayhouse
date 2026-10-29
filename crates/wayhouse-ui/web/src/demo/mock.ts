// The in-browser backend of the interactive demo: replaces fetch and WebSocket so the real UI runs
// with no server. State lives in memory and resets on reload. Only demo.html loads this file.
import YAML from "yaml";
import { createTwoFilesPatch } from "diff";
import schema from "../../../../wayhouse-config/schema/config.schema.json";
import type { FleetNode } from "../lib/fleet/nodes";
import type { FeatureOverride, PluginInstall, SnifferInfo } from "../types";
import { acmeSites, CONFIG_YAML, fleetView, FLAGS, GATEWAYS, LISTENERS, MODULE_SHA, nodes, PELICAN_MANIFEST, PLUGIN_CAPS, rng } from "./data";

type Body = unknown;
const ok = (b: Body = "ok", status = 200, headers: Record<string, string> = {}) =>
  new Response(typeof b === "string" ? b : JSON.stringify(b), { status, headers });
const err = (status: number, error: string, extra: object = {}) => ok({ error, ...extra }, status);
const sec = () => Math.floor(Date.now() / 1000);

// --- config revisions ---
const revs: { text: string; canary?: string[]; promoted: boolean }[] = [];
const edit = (t: string, from: string, to: string) => t.replace(from, to);
{
  let t = CONFIG_YAML;
  const steps: [string, string][] = [
    ["max_connections: 200000", "max_connections: 120000"],
    ["max_connections: 120000", "max_connections: 200000"],
    ["{ rate: 5, burst: 10 }", "{ rate: 10, burst: 20 }"],
    ["idle_timeout_sec: 60", "idle_timeout_sec: 45"],
    ["max_sessions: 500", "max_sessions: 800"],
    ["deny: [\"203.0.113.0/24\"]", "deny: [\"203.0.113.0/24\", \"198.51.100.0/24\"]"],
  ];
  revs.push({ text: edit(t, "max_connections: 200000", "max_connections: 100000"), promoted: true });
  for (const [a, b] of steps) revs.push({ text: (t = edit(t, a, b)), promoted: true });
  // Revision 8 is the current one, on trial.
  revs.push({ text: edit(t, "idle_timeout_sec: 90", "idle_timeout_sec: 120"), promoted: true });
}
let current = revs.length;
const AUTO_CONFIRM = 600;
let pending: null | { revision: number; revert_to: number; deadline_ms: number; reasons: string[] } = {
  revision: current, revert_to: current - 1, deadline_ms: Date.now() + AUTO_CONFIRM * 1000, reasons: ["pool-timeouts", "proxy-protocol"],
};
const RISKS = [
  { id: "pool-timeouts", text: "A pool's idle timeout changed: connections idle between the old and new limit may be dropped." },
  { id: "proxy-protocol", text: "The survival pool sends PROXY protocol v2: a backend that does not expect it will reject every connection." },
];
function settle() {
  if (pending && Date.now() > pending.deadline_ms) revert(pending.revision);
}
function revert(rev: number) {
  if (!pending || pending.revision !== rev) return false;
  revs.push({ text: revs[pending.revert_to - 1].text, promoted: true });
  current = revs.length;
  pending = null;
  return true;
}
const revisionList = () =>
  revs.map((r, i) => ({ revision: i + 1, size_bytes: r.text.length, current: i + 1 === current, promoted: r.promoted, ...(r.canary ? { canary_groups: r.canary } : {}) }));

// --- node tags, saved filters, themes, dashboards ---
const filters: any[] = [{ name: "Degraded in the EU", state: "degraded", region: "eu-central" }, { name: "Premium nodes", tags: ["tier=premium"] }];
const themes: any[] = [{ name: "Harbour", base: "dark", tokens: { "primary": "#3fb6a8" } }];
let defaults: any = {};
let layout: unknown = null;
const dashboards: any[] = [
  { name: "Overview", widgets: [{ id: "nodes.v1", width: 6 }, { id: "attention.v1", width: 6 }, { id: "connections.v1", width: 6 }, { id: "throughput.v1", width: 6 }, { id: "rollout.v1", width: 6 }, { id: "certificates.v1", width: 6 }] },
  { name: "Capacity", widgets: [{ id: "connections.v1", width: 12 }, { id: "throughput.v1", width: 12 }] },
];

// --- features ---
const overrides = new Map<string, FeatureOverride>([
  ["geo", { name: "geo", value: false, reason: "GeoIP database refresh is broken, see INC-212", actor: "admin", set_at: sec() - 3600, expires_at: sec() + 7200 }],
]);
const configFeatures: Record<string, boolean> = { sniffers: true, rate_limit: true };
const featureView = (scope: "proxy" | "controller") =>
  FLAGS.filter((x) => x.scope === scope).map((x) => {
    const o = overrides.get(x.name);
    const inCfg = x.name in configFeatures;
    return { ...x, value: o ? o.value : inCfg ? configFeatures[x.name] : x.default, source: o ? "override" : inCfg ? "config" : "default", available: true };
  });

// --- plugins ---
let pluginSeq = 3;
const plugins: PluginInstall[] = [
  { id: "p1", name: "pelican-panel", sha256: MODULE_SHA, size: 412_330, approved: PLUGIN_CAPS.pelican, config: { panel_url: "https://panel.example.net", node_ids: [1, 2], port_range: "30000-30999", sync_allocations: true }, enabled: true, created_at: sec() - 86400 * 12, created_by: "admin", webhook: { path: "/hooks/p1" } },
  { id: "p2", name: "cloudflare-dns", sha256: MODULE_SHA.replace(/^9f/, "3a"), size: 298_114, approved: PLUGIN_CAPS.cloudflare, config: {}, enabled: true, created_at: sec() - 86400 * 30, created_by: "admin" },
  { id: "p3", name: "hcloud-autoscale", sha256: MODULE_SHA.replace(/^9f/, "c7"), size: 355_902, approved: PLUGIN_CAPS.hcloud, config: {}, enabled: false, created_at: sec() - 86400 * 3, created_by: "admin" },
];
const secretsSet = new Map<string, Set<string>>([["p1", new Set(["api_token"])], ["p2", new Set(["cf_token"])]]);
const portRoutes = Array.from({ length: 9 }, (_, i) => ({ key: `srv-${1042 + i}`, port: 30010 + i * 7, proto: (i % 3 === 0 ? "udp" : "tcp") as "udp" | "tcp", backend: `10.20.4.${20 + i}:25565`, owner: `server-${1042 + i}`, plugin: "pelican-panel" }));

// --- sniffers ---
const sniffers: SnifferInfo[] = [
  { name: "minecraft_handshake", sha256: "aa11".repeat(16), size_bytes: 48_211, loaded: true, has_previous: true },
  { name: "a2s_query", sha256: "bb22".repeat(16), size_bytes: 31_904, loaded: true },
  { name: "raknet_ping", sha256: "cc33".repeat(16), size_bytes: 29_117, loaded: true },
  { name: "tls_sni", sha256: "dd44".repeat(16), size_bytes: 52_660, loaded: true, fallback: true },
];
const REGISTRIES = [
  { id: "official", name: "Wayhouse official", url: "https://sniffers.wayhouse-proxy.dev/index.json", official: true, risk: "official" as const },
  { id: "community", name: "Community mirror", url: "https://sniffers.example.org/index.json", official: false, risk: "external" as const },
];
const sver = (version: string, n: number) => ({ version, abi: "0.1", min_proxy: "0.3.0", url: `https://sniffers.wayhouse-proxy.dev/${version}.wasm`, sha256: n.toString(16).repeat(64).slice(0, 64), size: 40_000 + n * 1000, verifiable: true, limits: { max_memory_bytes: 16_777_216, call_timeout_ms: 20 } });
const registrySniffers = (id: string) => ({
  registry: REGISTRIES.find((r) => r.id === id)!, index_name: id === "official" ? "wayhouse-official" : "community", host_abi: "0.1", min_proxy_checked: true,
  sniffers: [
    { name: "minecraft_handshake", description: "Minecraft Java handshake: host name and protocol version.", license: "MIT OR Apache-2.0", homepage: "https://github.com/wayhouse-proxy/sniffers", versions: [sver("0.1.1", 5), sver("0.1.0", 4)], verifiable: id === "official", compatible: { version: "0.1.1" } },
    { name: "a2s_query", description: "Source engine A2S queries (CS2, Rust, ARK).", license: "MIT OR Apache-2.0", versions: [sver("0.1.0", 6)], verifiable: id === "official", compatible: { version: "0.1.0" } },
    { name: "raknet_ping", description: "Bedrock RakNet unconnected ping.", license: "MIT OR Apache-2.0", versions: [sver("0.1.0", 7)], verifiable: id === "official", compatible: { version: "0.1.0" } },
    { name: "valheim_steam", description: "Valheim Steam query packets.", license: "MIT", versions: [sver("0.2.0", 8)], verifiable: id === "official", compatible: { reason: "needs wayhouse 0.5.0 or newer" } },
  ].slice(0, id === "official" ? 4 : 2),
});

// --- tunnel addresses ---
const tunnel = Array.from({ length: 10 }, (_, i) => ({
  role: (i < 6 ? "origin" : "proxy") as "origin" | "proxy",
  name: i < 6 ? ["home-lab", "hetzner-a", "hetzner-b", "ovh-bm1", "ovh-bm2", "office"][i] : nodes[i * 3].instance,
  address: `10.77.0.${10 + i}`, first_seen: sec() - 86400 * (40 - i), last_seen: sec() - (i === 5 ? 86400 * 3 : 20 * i), stale: i === 5,
}));

// --- stats ---
function stats(range: string) {
  const mins = range === "5m" ? 5 : range === "1h" ? 60 : 15;
  const nowMs = Date.now();
  const minute = Math.floor(nowMs / 60000);
  const mk = (scale: number, seed: number) => {
    const r = rng(seed);
    return Array.from({ length: mins }, (_, i) => {
      const m = minute - (mins - 1 - i);
      const wave = 1 + 0.35 * Math.sin(m / 23 + seed) + 0.12 * Math.sin(m / 5);
      return { m, k: wave * (0.92 + r() * 0.16) * scale, partial: i === mins - 1 };
    });
  };
  const series = <P,>(name: string | undefined, scale: number, seed: number, f: (k: number, m: number, partial: boolean) => P) => ({
    ...(name ? { name } : {}), resolution_sec: 60, last_sample_ms: nowMs - 12_000,
    points: mk(scale, seed).map((p) => ({ t_ms: p.m * 60000, partial: p.partial, ...f(p.k, p.m, p.partial) })),
  });
  const share = [0.36, 0.1, 0.18, 0.08, 0.06, 0.22];
  return {
    generated_at_ms: nowMs, range_sec: mins * 60,
    fleet: series(undefined, 1, 1, (k) => ({ accepted_per_sec: 310 * k, blocked_per_sec: 9 * k * (k > 1.2 ? 2.4 : 1), bytes_per_sec: 61e6 * k, packets_per_sec: 71_000 * k, tcp_sessions: Math.round(41_000 * k), udp_sessions: Math.round(63_000 * k) })),
    listeners: LISTENERS.map((n, i) => series(n, share[i], 10 + i, (k) => ({ accepted_per_sec: 310 * k, blocked_per_sec: (i === 0 ? 7 : 0.4) * k, bytes_c2s_per_sec: 9e6 * k, bytes_s2c_per_sec: 52e6 * k, packets_per_sec: 71_000 * k }))),
    pools: ["survival", "lobby", "bedrock", "valheim", "voice"].map((n, i) => series(n, 1, 30 + i, (_k, m) => {
      const total = [4, 3, 3, 2, 2][i] * nodes.length;
      const down = n === "survival" ? (m % 90 > 70 ? 6 : 3) : n === "voice" ? 1 : 0;
      return { backends: total, healthy_backends: total - down };
    })),
    regions: ["eu-central", "eu-west", "us-east", "us-west", "ap-southeast", "ap-northeast", "sa-east", "oc-east"].map((n, i) => series(n, [0.9, 0.7, 1, 0.6, 0.35, 0.3, 0.15, 0.2][i], 50 + i, (k) => ({ bytes_per_sec: 9e6 * k }))),
  };
}

// --- fleet nodes ---
function nodePage(p: URLSearchParams) {
  const q = (p.get("q") ?? "").toLowerCase(), state = p.get("state"), region = p.get("region"), tags = p.getAll("tag");
  let list = nodes.filter((n) =>
    (!q || n.instance.includes(q) || Object.entries(n.tags).some(([k, v]) => `${k}=${v}`.includes(q))) &&
    (!state || n.state === state) && (!region || n.region === region) &&
    tags.every((t) => { const [k, v] = t.split("="); return k in n.tags && (v === undefined || n.tags[k] === v); }));
  const dir = p.get("dir") === "desc" ? -1 : 1, sort = p.get("sort") ?? "instance";
  const key = (n: FleetNode): string | number => sort === "sessions" ? n.sessions.tcp + n.sessions.udp : sort === "state" ? n.state : sort === "region" ? n.region ?? "" : sort === "version" ? n.version : n.instance;
  list = [...list].sort((a, b) => (key(a) < key(b) ? -dir : key(a) > key(b) ? dir : 0));
  const roll = (l: FleetNode[]) => ({ nodes: l.length, degraded: l.filter((n) => n.state === "degraded").length, stale: l.filter((n) => n.state === "stale").length, sessions: l.reduce((s, n) => s + n.sessions.tcp + n.sessions.udp, 0) });
  const size = Number(p.get("size")) || 25, page = Number(p.get("page")) || 1;
  const regions = [...new Set(list.map((n) => n.region ?? ""))];
  return { items: list.slice((page - 1) * size, page * size), total: list.length, page, size, rollup: roll(list), groups: regions.map((r) => ({ region: r || null, ...roll(list.filter((n) => (n.region ?? "") === r)) })) };
}

const fanout = () => ({ results: nodes.map((n) => ({ instance: n.instance, status: n.state === "stale" ? null : 200, error: n.state === "stale" ? "timed out after 2s" : null })) });

async function route(method: string, url: URL, req: Request): Promise<Response> {
  const path = url.pathname, p = url.searchParams;
  const m = (re: RegExp) => path.match(re);
  const text = () => req.text();
  const json = async () => JSON.parse((await req.text()) || "{}");
  let r: RegExpMatchArray | null;

  if (path === "/ui/session") return ok({ username: "demo", role: "admin" });
  if (path === "/ui/login" || path === "/ui/logout") return ok("ok");

  // fleet
  if (path === "/api/fleet/nodes/resolve") { const l = nodePage(new URLSearchParams(`${p}&size=1000`)); return ok({ count: l.total, instances: l.items.map((n) => n.instance) }); }
  if (path === "/api/fleet/nodes") return ok(nodePage(p));
  if ((r = m(/^\/api\/fleet\/nodes\/([^/]+)\/tags$/))) {
    const n = nodes.find((x) => x.instance === decodeURIComponent(r![1]));
    if (!n) return err(404, "unknown node");
    const tags = method === "PUT" ? (await json()).tags : {};
    n.operator_tags = tags;
    const declared = n.tags; // keep declared tags that the operator did not override
    n.tags = { ...Object.fromEntries(Object.entries(declared).filter(([k]) => !(n.operator_tags && k in n.operator_tags) && !k.startsWith("__"))), ...tags };
    return ok("");
  }
  if (path === "/api/fleet/stats") return ok(stats(p.get("range") ?? "15m"));
  if (/^\/api\/fleet\/instances\/[^/]+\/(un)?drain$/.test(path)) return ok("ok");
  if ((r = m(/^\/api\/fleet\/instances\/([^/]+)\/sniffers$/))) return ok(sniffers);
  if (path === "/api/fleet/sniffers" && method === "POST") { sniffers.push({ name: p.get("name")!, sha256: "ee55".repeat(16), size_bytes: (await req.arrayBuffer()).byteLength, loaded: true }); return ok(fanout()); }
  if ((r = m(/^\/api\/fleet\/sniffers\/([^/]+)(\/rollback)?$/))) {
    const i = sniffers.findIndex((s) => s.name === decodeURIComponent(r![1]));
    if (i >= 0 && method === "DELETE") sniffers.splice(i, 1);
    if (i >= 0 && r[2]) sniffers[i].has_previous = false;
    return ok(fanout());
  }
  if (path === "/api/fleet/route-hint" || path.startsWith("/api/fleet/pools/")) return ok(fanout());

  // sniffer registries
  if (path === "/api/registries" && method === "GET") return ok({ registries: REGISTRIES, persistent: false });
  if (path === "/api/registries" && method === "POST") { const b = await json(); const ref = { id: `r${REGISTRIES.length}`, name: b.name ?? new URL(b.url).host, url: b.url, official: false, risk: "external" as const }; REGISTRIES.push(ref); return ok(ref); }
  if (path === "/api/registries/updates/check") {
    return ok({ host_abi: "0.1", min_proxy_checked: true,
      registries: [{ id: "official", name: "Wayhouse official", ok: true }, { id: "community", name: "Community mirror", ok: false, error: "connection refused" }],
      sniffers: [
        { sniffer: "minecraft_handshake", installed_sha256: sniffers[0]?.sha256 ?? "", installed_version: "0.1.0", known: true, has_previous: true, fallback: false, update: { registry_id: "official", version: "0.1.1", compatible: true, reason: null }, instances: nodes.slice(0, 32).map((n) => n.instance) },
        { sniffer: "tls_sni", installed_sha256: "dd44".repeat(16), installed_version: null, known: false, has_previous: false, fallback: true, update: null, instances: nodes.slice(0, 3).map((n) => n.instance) },
      ], instance_errors: [{ instance: nodes[17].instance, status: 504, detail: "no answer from the admin API" }] });
  }
  if ((r = m(/^\/api\/registries\/([^/]+)\/sniffers$/))) return ok(registrySniffers(r[1]));
  if ((r = m(/^\/api\/registries\/([^/]+)\/install$/))) {
    const b = await json();
    return ok({ sniffer: b.name, version: b.version ?? "0.1.1", signed: r[1] === "official", risk: r[1] === "official" ? "official" : "external", sha256: "ab".repeat(32),
      results: nodes.map((n) => ({ instance: n.instance, ok: n.state !== "stale", pinned: false, error: n.state === "stale" ? "unreachable" : null, status: n.state === "stale" ? null : 200 })), pinned_instances: [] }, 207);
  }
  if ((r = m(/^\/api\/registries\/([^/]+)$/)) && method === "DELETE") { const i = REGISTRIES.findIndex((x) => x.id === r![1]); if (i > 0) REGISTRIES.splice(i, 1); return ok(""); }

  // plugins
  if (path === "/api/plugin-routes") return ok({ routes: [], conflicts: [], port_routes: portRoutes, exhausted: [{ key: "srv-1099", backend: "10.20.4.99:25565", owner: "server-1099", plugin: "pelican-panel" }] });
  if (path === "/api/plugins/modules") { const n = (await req.arrayBuffer()).byteLength; return ok({ sha256: MODULE_SHA, size: n, abi: "0.2", capabilities: { ...PLUGIN_CAPS.pelican, http: null, secrets: [], port_routes: null }, manifest: PELICAN_MANIFEST }); }
  if (path === "/api/plugins" && method === "GET") return ok(plugins);
  if (path === "/api/plugins" && method === "POST") {
    const b = await json();
    const eff = { ...PLUGIN_CAPS.pelican, http: { hosts: [{ host: new URL(b.config?.panel_url ?? "https://panel.example.net").host }] }, secrets: [{ name: "api_token", description: "Pelican application API token", hosts: [new URL(b.config?.panel_url ?? "https://panel.example.net").host] }] };
    if (b.dry_run) return ok({ effective: eff });
    const inst: PluginInstall = { id: `p${++pluginSeq}`, name: b.name, sha256: b.sha256, size: 412_330, approved: b.approved, config: b.config ?? {}, enabled: b.enabled, created_at: sec(), created_by: "demo" };
    plugins.push(inst);
    return ok(inst);
  }
  if ((r = m(/^\/api\/plugins\/([^/]+)(?:\/(.+))?$/))) {
    const pl = plugins.find((x) => x.id === r![1]);
    if (!pl) return err(404, "no such install");
    const sub = r[2];
    if (!sub) { if (method === "DELETE") { plugins.splice(plugins.indexOf(pl), 1); return ok(""); } return ok({ ...pl, manifest: pl.name === "pelican-panel" ? PELICAN_MANIFEST : null }); }
    if (sub === "status") return ok({ ticks: 4821, last_tick_unix: sec() - 14, last_ok: true, last_error: pl.name === "hcloud-autoscale" ? "429 from api.hetzner.cloud, backing off" : null, consecutive_failures: pl.name === "hcloud-autoscale" ? 2 : 0, held: null, hook_calls: 311, hook_failures: 2, hooks_dropped: 0, logs: [{ level: "info", msg: "synced 41 allocations from panel" }, { level: "info", msg: "added 2 port routes" }, { level: "warn", msg: "allocation 1099 has no free entry port" }, { level: "info", msg: "tick done in 182ms" }] });
    if (sub === "config") { const b = await json(); pl.config = b.config; if (b.approved) pl.approved = b.approved; return ok(pl); }
    if (sub === "secrets") return ok({ secrets: (pl.approved.secrets ?? []).map((s) => { const set = secretsSet.get(pl.id)?.has(s.name) ?? false; return { slot: s.name, description: s.description, bound_hosts: s.hosts, set, status: set ? "ok" : "unset", key_id: set ? "cluster-key-1" : null, updated_at: set ? sec() - 86400 * 4 : null }; }) });
    if ((r = sub.match(/^secrets\/(.+)$/))) { const s = secretsSet.get(pl.id) ?? new Set<string>(); method === "PUT" ? s.add(decodeURIComponent(r[1])) : s.delete(decodeURIComponent(r[1])); secretsSet.set(pl.id, s); return ok(""); }
    if (sub === "webhook") { if (method === "DELETE") { delete pl.webhook; return ok(""); } pl.webhook = { path: `/hooks/${pl.id}` }; return ok({ token: "whk_demo_" + Math.random().toString(36).slice(2, 18), path: `/hooks/${pl.id}` }); }
    if (sub === "enable" || sub === "disable") { pl.enabled = sub === "enable"; return ok(pl); }
  }

  // config
  if (path === "/api/config/schema") return ok(schema);
  if (path === "/api/config/pending") { settle(); const now_ms = Date.now(); return ok({ pending: pending && { ...pending, remaining_ms: Math.max(0, pending.deadline_ms - now_ms) }, now_ms, auto_confirm_secs: AUTO_CONFIRM, risks: RISKS }); }
  if (path === "/api/config/validate") { try { const d = YAML.parse(await text()); if (typeof d !== "object" || !d) throw new Error("the document must be a mapping"); return ok({ ok: true, schema_version: d.schema_version ?? 1 }); } catch (e) { return err(422, String((e as Error).message)); } }
  if (path === "/api/config" && method === "GET") { settle(); return ok(revs[current - 1].text, 200, { "X-Config-Revision": String(current), "content-type": "text/yaml" }); }
  if (path === "/api/config" && method === "POST") {
    settle();
    const body = await text();
    const base = p.get("base");
    if (base && Number(base) !== current) return err(409, `revision ${current} landed since revision ${base}`);
    try { YAML.parse(body); } catch (e) { return err(422, String((e as Error).message)); }
    const canary = p.get("stage") === "canary";
    revs.push({ text: body, promoted: !canary, ...(canary ? { canary: [p.get("group")!] } : {}) });
    const wait = Number(p.get("confirm_within") ?? AUTO_CONFIRM);
    if (!canary) { const prev = current; current = revs.length; if (wait > 0) pending = { revision: current, revert_to: prev, deadline_ms: Date.now() + wait * 1000, reasons: [] }; }
    return ok({ revision: revs.length });
  }
  if ((r = m(/^\/api\/config\/confirm\/(\d+)$/))) { if (pending?.revision === Number(r[1])) pending = null; return ok(""); }
  if (path === "/api/config/revert") return revert(Number(p.get("revision"))) ? ok("") : err(409, "another revision is on trial");
  if ((r = m(/^\/api\/config\/promote\/(\d+)$/))) { const rv = revs[Number(r[1]) - 1]; rv.promoted = true; delete rv.canary; current = Number(r[1]); return ok(""); }
  if (path === "/api/config/revisions") return ok(revisionList());
  if ((r = m(/^\/api\/config\/revisions\/(\d+)\/diff$/))) { const n = Number(r[1]), a = p.get("against") ? Number(p.get("against")) : n - 1; return ok(createTwoFilesPatch(`revision ${a}`, `revision ${n}`, revs[a - 1]?.text ?? "", revs[n - 1].text), 200); }
  if ((r = m(/^\/api\/config\/revisions\/(\d+)$/))) return revs[Number(r[1]) - 1] ? ok(revs[Number(r[1]) - 1].text) : err(404, "no such revision");
  if ((r = m(/^\/api\/config\/rollback\/(\d+)$/))) { revs.push({ text: revs[Number(r[1]) - 1].text, promoted: true }); current = revs.length; return ok({ revision: current }); }

  // tunnel
  if (path === "/api/tunnel/addresses") return ok({ network: "10.77.0.0/20", allocated: tunnel.length, capacity: 4094, entries: tunnel });
  if ((r = m(/^\/api\/tunnel\/(origins|proxies)\/(.+)$/))) { const i = tunnel.findIndex((t) => t.name === decodeURIComponent(r![2])); const rel = tunnel[i]?.address ?? null; if (i >= 0) tunnel.splice(i, 1); return ok({ revision: 1, released: rel }); }

  // acme
  const sites = acmeSitesState();
  if (path === "/api/acme/sites" && method === "GET") return ok({ sites });
  if ((r = m(/^\/api\/acme\/sites\/(.+)$/))) {
    const name = decodeURIComponent(r[1]);
    if (method === "DELETE") { const i = sites.findIndex((s) => s.name === name); if (i >= 0) sites.splice(i, 1); return ok(""); }
    const b = await json(); const i = sites.findIndex((s) => s.name === name);
    const next = { ...(sites[i] ?? { certificate: null, attempt: null }), name, names: b.names, challenge: b.challenge, dns_plugin: b.dns_plugin ?? "", challenge_zone: b.challenge_zone ?? null, gateways: b.gateways, propagation_timeout_secs: b.propagation_timeout_secs ?? 120, enabled: b.enabled };
    if (i >= 0) sites[i] = next as any; else sites.push(next as any);
    return ok("");
  }
  if (path === "/api/acme/gateways" && method === "GET") return ok({ gateways: GATEWAYS.map((g) => ({ ...g, created_at: sec() - 86400 * 40 })) });
  if (path === "/api/acme/gateways" && method === "POST") { const b = await json(); GATEWAYS.push({ name: b.name, sites: [] }); return ok({ name: b.name, token: "gwt_demo_" + Math.random().toString(36).slice(2, 22) }); }
  if (path === "/api/acme/account") return ok({ directory_url: "https://acme-v02.api.letsencrypt.org/directory", account_url: "https://acme-v02.api.letsencrypt.org/acme/acct/1948201337", key_id: "cluster-key-1" });

  // features
  if (path === "/api/features" && method === "GET") return ok({ flags: featureView("controller"), proxy_flags: featureView("proxy") });
  if (path === "/api/features/overrides" && method === "GET") return ok({ items: [...overrides.values()] });
  if ((r = m(/^\/api\/features\/overrides\/(.+)$/))) {
    const name = decodeURIComponent(r[1]);
    if (method === "DELETE") { overrides.delete(name); return ok(""); }
    const b = await json();
    const fl = FLAGS.find((x) => x.name === name);
    if (!fl) return err(404, `unknown flag ${name}`);
    if (fl.override === "off_only" && b.value) return err(422, `${name} can only be switched off by an override`);
    if (fl.protection && !b.value && !b.expires_at) return err(422, `${name} removes protection: an override needs an expiry`);
    const o: FeatureOverride = { name, value: b.value, reason: b.reason, actor: "demo", set_at: sec(), ...(b.expires_at ? { expires_at: b.expires_at } : {}) };
    overrides.set(name, o);
    return ok(o);
  }

  // stored UI data
  if (path === "/api/ui/filters") return ok({ items: filters });
  if (path === "/api/ui/themes") return ok({ items: themes });
  if (path === "/api/ui/dashboards") return ok({ items: dashboards });
  if (path === "/api/ui/defaults") { if (method === "PUT") defaults = await json(); return ok(defaults); }
  if (path === "/api/ui/layout") {
    if (method === "GET") return layout ? ok(layout) : err(404, "no layout stored");
    if (method === "PUT") { try { layout = JSON.parse(await text()); } catch (e) { return err(422, String(e)); } return ok(layout as object); }
    layout = null; return ok("");
  }
  if ((r = m(/^\/api\/ui\/(filters|themes|dashboards)\/(.+)$/))) {
    const list = { filters, themes, dashboards }[r[1] as "filters"];
    const name = decodeURIComponent(r[2]);
    const i = list.findIndex((x: any) => x.name === name);
    if (method === "DELETE") { if (i >= 0) list.splice(i, 1); return ok(""); }
    const b = await json(); i >= 0 ? (list[i] = b) : list.push(b); return ok(b);
  }

  return err(404, `demo has no ${method} ${path}`);
}

let siteState: ReturnType<typeof acmeSites> | null = null;
const acmeSitesState = () => (siteState ??= acmeSites());

export function installMock() {
  const real = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const req = new Request(input, init);
    const url = new URL(req.url);
    if (url.origin !== new URL(document.baseURI).origin || !(url.pathname.startsWith("/api/") || url.pathname.startsWith("/ui/"))) return real(input, init);
    await new Promise((res) => setTimeout(res, 40 + Math.random() * 120)); // a little latency so loading states are visible
    try { return await route(req.method, url, req); } catch (e) { return err(500, String(e)); }
  };

  // /ws/fleet: a fake socket that pushes the fleet view now and every few seconds.
  class FakeSocket extends EventTarget {
    static OPEN = 1;
    readyState = 0;
    onopen: ((e: Event) => void) | null = null;
    onmessage: ((e: MessageEvent) => void) | null = null;
    onclose: ((e: CloseEvent) => void) | null = null;
    onerror: ((e: Event) => void) | null = null;
    private timer: ReturnType<typeof setInterval> | undefined;
    constructor(public url: string) {
      super();
      setTimeout(() => {
        this.readyState = 1;
        this.onopen?.(new Event("open"));
        const push = () => {
          const view = fleetView().map((v) => ({ ...v, last_seen_ms_ago: v.stale ? v.last_seen_ms_ago : Math.floor(Math.random() * 9000), sessions: v.stale ? v.sessions : { tcp: v.sessions.tcp + Math.floor((Math.random() - 0.5) * 80), udp: v.sessions.udp + Math.floor((Math.random() - 0.5) * 160) } }));
          this.onmessage?.(new MessageEvent("message", { data: JSON.stringify(view) }));
        };
        push();
        this.timer = setInterval(push, 4000);
      }, 150);
    }
    send() {}
    close() { clearInterval(this.timer); this.readyState = 3; }
  }
  (window as any).WebSocket = FakeSocket;
}
