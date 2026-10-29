// Fake deployment for the interactive demo (demo.html). Nothing here is real: names, addresses and
// certificates are invented. Time-based data is generated on request so the charts look live.
import type { FleetInstanceView, AcmeSite, FeatureFlag, PluginCapabilities } from "../types";
import type { FleetNode } from "../lib/fleet/nodes";

// Deterministic pseudo-random, so a reload shows the same fleet.
export function rng(seed: number) {
  let s = seed >>> 0 || 1;
  return () => ((s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 2 ** 32);
}

const REGIONS: { region: string; path: string; sites: string[]; provider: string }[] = [
  { region: "eu-central", path: "eu/frankfurt", sites: ["fra"], provider: "hetzner" },
  { region: "eu-west", path: "eu/amsterdam", sites: ["ams", "lon"], provider: "ovh" },
  { region: "us-east", path: "us/virginia", sites: ["iad", "nyc"], provider: "aws" },
  { region: "us-west", path: "us/oregon", sites: ["pdx", "sfo"], provider: "aws" },
  { region: "ap-southeast", path: "ap/singapore", sites: ["sin"], provider: "vultr" },
  { region: "ap-northeast", path: "ap/tokyo", sites: ["nrt"], provider: "vultr" },
  { region: "sa-east", path: "sa/saopaulo", sites: ["gru"], provider: "ovh" },
  { region: "oc-east", path: "oc/sydney", sites: ["syd"], provider: "ovh" },
];
const COUNTS = [7, 6, 7, 5, 4, 4, 3, 4]; // 40 nodes

export const nodes: FleetNode[] = [];
{
  const r = rng(7);
  REGIONS.forEach((g, gi) => {
    for (let i = 0; i < COUNTS[gi]; i++) {
      const site = g.sites[i % g.sites.length];
      const n = nodes.length;
      const state = [11, 23, 31].includes(n) ? "degraded" : [17, 36].includes(n) ? "stale" : "ok";
      const old = [5, 12, 27].includes(n);
      nodes.push({
        instance: `${site}-edge-${String(Math.floor(i / g.sites.length) + 1).padStart(2, "0")}`,
        state,
        last_seen_ms_ago: state === "stale" ? 240_000 + Math.floor(r() * 90_000) : Math.floor(r() * 9000),
        region: g.region,
        group: `${g.path}/cluster-${i % 2 ? "b" : "a"}`,
        tags: { provider: g.provider, tier: i % 3 === 0 ? "premium" : "standard", env: "prod", ...(n % 9 === 0 ? { canary: "true" } : {}) },
        operator_tags: n % 9 === 0 ? { canary: "true" } : n === 3 ? { note: "new-rack" } : {},
        version: old ? "0.3.0" : "0.4.0",
        protocol: old ? "1.3" : "1.4",
        skew: old ? (n === 27 ? "outside-window" : "within-window") : "none",
        sessions: state === "stale" ? { tcp: 0, udp: 0 } : { tcp: 400 + Math.floor(r() * 2600), udp: 900 + Math.floor(r() * 5200) },
        backends: 14,
        unhealthy_backends: state === "degraded" ? 2 + Math.floor(r() * 3) : 0,
      });
    }
  });
  // The operator's tags overlay the declared ones, as the aggregator does.
  nodes.forEach((n) => (n.tags = { ...n.tags, ...n.operator_tags }));
}

const POOLS: { name: string; balancer: string; port: number; n: number }[] = [
  { name: "survival", balancer: "least_conn", port: 25565, n: 4 },
  { name: "lobby", balancer: "round_robin", port: 25566, n: 3 },
  { name: "bedrock", balancer: "consistent_hash", port: 19132, n: 3 },
  { name: "valheim", balancer: "consistent_hash", port: 2456, n: 2 },
  { name: "voice", balancer: "round_robin", port: 9987, n: 2 },
];

export function fleetView(): FleetInstanceView[] {
  return nodes.map((nd, ni) => {
    const bad = nd.unhealthy_backends;
    let k = 0;
    return {
      instance: nd.instance,
      last_seen_ms_ago: nd.last_seen_ms_ago,
      stale: nd.state === "stale",
      group: nd.group,
      version: nd.version,
      protocol: nd.protocol,
      skew: nd.skew,
      sessions: nd.sessions,
      pools: POOLS.map((p) => ({
        name: p.name,
        balancer: p.balancer,
        backends: Array.from({ length: p.n }, (_, i) => {
          const down = k++ < bad;
          return {
            addr: `10.${20 + (ni % 8)}.${p.n}.${10 + i}:${p.port}`,
            healthy: !down,
            state: i === p.n - 1 && p.name === "lobby" && ni % 5 === 0 ? "draining" : "enabled",
            active: down ? 0 : 20 + ((ni * 7 + i * 13) % 140),
          };
        }),
      })),
    };
  });
}

export const LISTENERS = ["minecraft-java", "minecraft-lobby", "minecraft-bedrock", "valheim", "voice-ts", "https-edge"];

// The base configuration the editors show. Keep it valid against crates/wayhouse-config/schema.
export const CONFIG_YAML = `# Wayhouse demo deployment: invented names and addresses.
schema_version: 4

settings:
  workers: 0
  shutdown_grace_sec: 30
  admin:
    listen: "127.0.0.1:9900"
  limits:
    max_connections: 200000
    max_udp_sessions: 200000
    max_new_sessions_per_sec: 20000
  group: "eu/frankfurt/cluster-a"

features:
  sniffers: true
  rate_limit: true

backend_sources:
  - name: dc-static
    type: static
    targets: ["10.20.1.10:25565", "10.20.1.11:25565"]
  - name: survival-srv
    type: dns_srv
    record: "_minecraft._tcp.survival.internal.example.net"
    refresh_interval_sec: 15
  - name: voice-consul
    type: consul
    service: "teamspeak"
    consul_addr: "http://127.0.0.1:8500"
    tag: "prod"
    refresh_interval_sec: 10
  - name: bedrock-k8s
    type: kubernetes
    service: "bedrock"
    namespace: "games"
    refresh_interval_sec: 10
  - name: home-origin
    type: tunnel
    pubkey: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
    refresh_interval_sec: 10

pools:
  - name: survival
    source: survival-srv
    balancer: least_conn
    connect_timeout_ms: 300
    idle_timeout_sec: 90
    proxy_protocol: v2
    health_check: { type: tcp_connect, interval_sec: 2, timeout_ms: 500, rise: 2, fall: 3 }
    per_backend: { max_sessions: 500 }
  - name: lobby
    source: dc-static
    balancer: round_robin
    connect_timeout_ms: 300
    idle_timeout_sec: 60
    health_check: { type: tcp_connect, interval_sec: 2, timeout_ms: 500, rise: 2, fall: 3 }
  - name: bedrock
    source: bedrock-k8s
    balancer: consistent_hash
    hash_on: src_ip
    idle_timeout_sec: 30
    health_check: { type: udp_probe, send_hex: "01", expect_hex_prefix: "1c", interval_sec: 2, timeout_ms: 500, rise: 2, fall: 3 }
  - name: valheim
    targets: ["10.23.2.10:2456", "10.23.2.11:2456"]
    balancer: consistent_hash
    hash_on: src_ip
    idle_timeout_sec: 30
    health_check: { type: none }
  - name: voice
    source: voice-consul
    balancer: round_robin
    idle_timeout_sec: 30
    health_check: { type: none }
  - name: web
    targets: ["127.0.0.1:8080"]
    balancer: round_robin
    health_check: { type: tcp_connect, interval_sec: 5, timeout_ms: 500, rise: 2, fall: 3 }

resolvers:
  - name: matchmaker
    type: http
    endpoint: "http://127.0.0.1:8099/resolve"
    timeout_ms: 40
    on_error: stale_ok
    cache:
      key: ["src_ip"]
      positive_ttl_sec: 30
      negative_ttl_sec: 2
      max_entries: 100000

listeners:
  - name: minecraft-java
    bind: "0.0.0.0:25565"
    protocol: tcp
    rate_limit:
      per_ip: { rate: 5, burst: 10 }
      per_net: { rate: 100, burst: 200 }
    per_source: { max_per_ip: 8, max_per_net: 256 }
    deny: ["203.0.113.0/24"]
    routes:
      - match: { type: sni, host: ["lobby.example.net"] }
        action: { pool: lobby }
      - match: { type: always }
        action: { pool: survival }
  - name: minecraft-lobby
    bind: "0.0.0.0:25566"
    protocol: tcp
    pool: lobby
  - name: minecraft-bedrock
    bind: "0.0.0.0:19132"
    protocol: udp
    pool: bedrock
    route_hint: true
  - name: valheim
    bind: "0.0.0.0:2456-2458"
    protocol: udp
    routes:
      - match: { type: always }
        action: { resolver: matchmaker }
  - name: voice-ts
    bind: "0.0.0.0:9987"
    protocol: udp
    pool: voice
  - name: https-edge
    bind: "0.0.0.0:443"
    protocol: tcp
    pool: web

web:
  listen: { http: "127.0.0.1:8080", https: "127.0.0.1:8443" }
  sites:
    - hosts: ["panel.example.net"]
      upstreams:
        - { addr: "10.20.9.5:8080" }
    - hosts: ["map.example.net", "dynmap.example.net"]
      upstreams:
        - { pool: survival, via: wayhouse }
      health_path: /up
`;

const DAY = 86400;
const now = () => Math.floor(Date.now() / 1000);
export function acmeSites(): AcmeSite[] {
  const t = now();
  const cert = (n: string[], daysLeft: number, extra: Partial<NonNullable<AcmeSite["certificate"]>> = {}) => ({
    version: 3,
    not_after: t + daysLeft * DAY,
    issued_at: t + (daysLeft - 90) * DAY,
    serial: "04:" + Array.from({ length: 8 }, (_, i) => ((n[0].length * 37 + i * 91) % 256).toString(16).padStart(2, "0")).join(":"),
    names: n,
    key_id: "cluster-key-1",
    held: null,
    untrusted: false,
    ...extra,
  });
  const base = { gateways: ["gw-fra", "gw-iad"], challenge: "dns-01" as const, dns_plugin: "cloudflare-dns", enabled: true, challenge_zone: null, propagation_timeout_secs: 120, attempt: null };
  return [
    { ...base, name: "panel", names: ["panel.example.net"], certificate: cert(["panel.example.net"], 61) },
    { ...base, name: "wildcard", names: ["*.example.net", "example.net"], certificate: cert(["*.example.net", "example.net"], 44) },
    { ...base, name: "map", names: ["map.example.net", "dynmap.example.net"], gateways: ["gw-fra"], challenge: "http-01", dns_plugin: "", certificate: cert(["map.example.net", "dynmap.example.net"], 6), attempt: { failures: 2, next_attempt_at: t + 1800, last_error: "http-01: connection to gw-fra timed out" } },
    { ...base, name: "staging-test", names: ["staging.example.net"], certificate: cert(["staging.example.net"], 80, { untrusted: true }) },
    { ...base, name: "billing", names: ["billing.example.net"], gateways: ["gw-iad"], certificate: cert(["billing.example.net"], 29, { held: "sealed with key cluster-key-0, which this controller no longer holds" }) },
    { ...base, name: "new-site", names: ["play.example.net"], enabled: false, certificate: null },
  ];
}
export const GATEWAYS = [
  { name: "gw-fra", sites: ["panel", "wildcard", "map"] },
  { name: "gw-iad", sites: ["panel", "wildcard", "billing"] },
  { name: "gw-sin", sites: [] as string[] },
];

type F = Omit<FeatureFlag, "value" | "source" | "available">;
const f = (name: string, scope: F["scope"], override: F["override"], risk: string, impact: string, protection = false, wired = true, apply: F["apply"] = "live"): F => ({
  name, scope, override, risk, impact, protection, wired, apply, default: true, since: 4,
});
export const FLAGS: F[] = [
  f("gossip_verdicts", "proxy", "both", "Backends are judged by this proxy's own health checks only; regional correlation is lost.", "The mesh keeps running; established connections are untouched."),
  f("sniffers", "proxy", "both", "Routes with `sniffer:` never match, so players can land on the default or another pool.", "Connections already routed are untouched; sniffing only reads the first bytes."),
  f("port_routes", "proxy", "off_only", "Plugin entry listeners are removed; customers lose those ports (they stay reserved).", "Established TCP sessions continue; UDP sessions drain.", false, true, "startup"),
  f("admin_write.backends", "proxy", "both", "Adding, patching and removing backends through the admin API answers 403.", "No connection is touched; reads, drain and the flag endpoints stay open."),
  f("admin_write.sniffer_upload", "proxy", "both", "Uploading, deleting and rolling back sniffer modules answers 403.", "No connection is touched; reads, drain and the flag endpoints stay open."),
  f("admin_write.route_hint", "proxy", "both", "Route hints answer 403.", "No connection is touched; reads, drain and the flag endpoints stay open."),
  f("rate_limit", "proxy", "both", "New-connection rate limits are bypassed: less protection against floods.", "New connections only.", true),
  f("per_source", "proxy", "both", "Per-source connection limits are bypassed: less protection against one noisy client.", "New connections only.", true),
  f("acl", "proxy", "both", "Client address allow and deny lists are bypassed.", "New connections only.", true),
  f("geo", "proxy", "both", "Country allow and deny lists are bypassed.", "New connections only.", true, false),
  f("plugin_webhooks", "controller", "off_only", "Every plugin webhook call answers 503; the listener stays up.", "No plugin tick is affected."),
  f("registry_install", "controller", "off_only", "Installing or updating from a registry is refused.", "Installed plugins and sniffers keep running."),
  f("acme_issuance", "controller", "both", "No new orders or renewals; served certificates keep working until they expire.", "No connection is touched."),
  f("tunnel_registration", "controller", "both", "New origin and proxy registrations are refused.", "Existing leases and tunnels are untouched."),
];

const caps = (p: Partial<PluginCapabilities>): PluginCapabilities => ({ triggers: {}, tick_interval_secs: 60, log: true, state: null, ...p });
export const PLUGIN_CAPS = {
  pelican: caps({
    triggers: { on_timer: true, on_webhook: true },
    tick_interval_secs: 30,
    state: { max_bytes: 65536 },
    http: { hosts: [{ host: "panel.example.net" }] },
    secrets: [{ name: "api_token", description: "Pelican application API token", hosts: ["panel.example.net"] }],
    port_routes: { ports: ["30000-30999"], proto: ["tcp", "udp"], backends: ["10.20.0.0/16"], max_entries: 400 },
  }),
  cloudflare: caps({
    triggers: { on_acme_dns: true },
    http: { hosts: [{ host: "api.cloudflare.com" }] },
    secrets: [{ name: "cf_token", description: "Cloudflare DNS edit token", hosts: ["api.cloudflare.com"] }],
  }),
  hcloud: caps({
    triggers: { on_timer: true, on_event: ["node_degraded"] },
    tick_interval_secs: 120,
    state: { max_bytes: 16384 },
    http: { hosts: [{ host: "api.hetzner.cloud" }] },
    secrets: [{ name: "hcloud_token", description: "Hetzner Cloud project token", hosts: ["api.hetzner.cloud"] }],
  }),
};

export const PELICAN_MANIFEST = {
  name: "pelican-panel",
  options: {
    type: "object",
    required: ["panel_url", "node_ids"],
    properties: {
      panel_url: { type: "string", title: "Panel URL", description: "Base URL of the Pelican panel.", format: "uri", default: "https://panel.example.net" },
      node_ids: { type: "array", title: "Node IDs", description: "Panel nodes whose servers get an entry port.", items: { type: "integer" }, default: [1, 2] },
      port_range: { type: "string", title: "Entry port range", default: "30000-30999" },
      sync_allocations: { type: "boolean", title: "Sync allocations", default: true },
      api_token: { type: "string", title: "API token", "x-wayhouse-secret": true },
    },
  },
  bindings: [{ option: "panel_url", kind: "http_host" }, { option: "port_range", kind: "port_routes" }],
};
export const MODULE_SHA = "9f2c1ab4e0d7" + "5b6c8e3a41f09d27c6b5a8e1d3f40927ab6c5d4e3f2a1b09";
