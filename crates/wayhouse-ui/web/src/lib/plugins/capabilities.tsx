import type { PluginCapabilities } from "../../types";

export function bytes(n: number): string {
  if (n >= 1024 * 1024) return `${+(n / (1024 * 1024)).toFixed(1)} MiB`;
  if (n >= 1024) return `${+(n / 1024).toFixed(1)} KiB`;
  return `${n} B`;
}

const KNOWN_KEYS = new Set(["triggers", "tick_interval_secs", "log", "state", "http", "secrets", "routes", "port_routes"]);
const KNOWN_TRIGGERS = new Set(["on_timer", "on_webhook", "on_event", "on_acme_dns"]);

/** Top-level keys (and trigger names) of a declared set that nothing here has wording for. */
export function unknownCapabilityKeys(caps: PluginCapabilities): string[] {
  const unknown = Object.keys(caps).filter((k) => !KNOWN_KEYS.has(k));
  for (const t of Object.keys(caps.triggers ?? {})) if (!KNOWN_TRIGGERS.has(t)) unknown.push(`triggers.${t}`);
  return unknown;
}

/** What a capability set lets a plugin do, in plain words. */
export function describeCapabilities(caps: PluginCapabilities): string[] {
  const out: string[] = [];
  const t = caps.triggers ?? {};
  if (t.on_timer) out.push(`Runs every ${caps.tick_interval_secs} s`);
  if (t.on_webhook) out.push("Can be called through a webhook you enable for it");
  if (t.on_event?.length) out.push(`Is told about wayhouse events: ${t.on_event.join(", ")}`);
  if (t.on_acme_dns) out.push("Sets and removes the DNS records certificate validation needs");
  if (caps.log) out.push("Writes log lines");
  if (caps.state) out.push(`Keeps up to ${bytes(caps.state.max_bytes)} of state`);
  for (const h of caps.http?.hosts ?? []) {
    const port = h.port && h.port !== 443 ? `:${h.port}` : "";
    out.push(`Makes HTTPS requests to ${h.host}${port}${h.allow_private ? ", including private and local addresses" : ""}`);
  }
  for (const s of caps.secrets ?? []) {
    out.push(`Sends the secret ${s.name} (${s.description}) only to ${s.hosts.join(", ")}`);
  }
  if (caps.routes) {
    const r = caps.routes;
    out.push(`Claims up to ${r.max_entries} hostnames (${r.hosts.join(", ")}) pointing into ${r.backends.join(", ")}`);
  }
  if (caps.port_routes) {
    const pr = caps.port_routes;
    out.push(
      `Publishes up to ${pr.max_entries} port routes on entry ports ${pr.ports.join(", ")} (${pr.proto.join(", ")}) to backends in ${pr.backends.join(", ")}`,
    );
  }
  // The whole declared object is what gets approved, so anything without wording must still be
  // shown rather than approved unseen.
  for (const key of unknownCapabilityKeys(caps)) out.push(`Also asks for: ${key}`);
  if (out.length === 0) out.push("Asks for nothing");
  return out;
}

export function CapabilityList({ caps }: { caps: PluginCapabilities }) {
  return (
    <ul className="list-disc pl-5 text-sm text-ink-muted">
      {describeCapabilities(caps).map((line) => (
        <li key={line}>{line}</li>
      ))}
    </ul>
  );
}
