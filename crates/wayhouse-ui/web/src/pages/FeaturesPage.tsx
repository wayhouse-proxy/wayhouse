import { useCallback, useEffect, useState } from "react";
import { parse } from "yaml";
import { ApiError, clearFeatureOverride, getCurrentConfig, getFeatures, getFleetFeatures, listFeatureOverrides, putFeatureOverride } from "../api";
import type { FeatureFlag, FeatureOverride, FlagRollup } from "../types";
import { AlertBar, PageHeader } from "../components/shell/Page";
import { ErrorState, Loading } from "../components/shell/States";
import { Badge } from "../components/ui/Badge";
import { Button } from "../components/ui/Button";
import { Dialog } from "../components/ui/Dialog";
import { Select, Textarea } from "../components/ui/Input";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { useCan } from "../lib/session";

/** How long an override lasts; a protection flag cannot be left off for good. */
const TTLS: [string, number][] = [
  ["15 minutes", 900],
  ["1 hour", 3600],
  ["8 hours", 28800],
  ["24 hours", 86400],
];

/** Plain names for the flags; one this build does not know shows its key alone. */
const LABELS: Record<string, string> = {
  gossip_verdicts: "Shared verdicts",
  sniffers: "Protocol sniffers",
  port_routes: "Port routes",
  rate_limit: "Rate limiting",
  per_source: "Per-source limits",
  acl: "Access lists",
  geo: "Geo rules",
  plugin_webhooks: "Plugin webhooks",
  acme_issuance: "Certificate issuance",
  registry_install: "Registry install",
  tunnel_registration: "Tunnel registration",
};

const SOURCE_LABEL = { default: "default", config: "config", override: "override", build: "not in this build" };

/** Which nodes run the flag differently; nothing to say when one node reports or all agree. */
function fleetNote(r: FlagRollup | undefined, reporting: number): { lines: string[]; names: string } | null {
  if (!r || reporting < 2) return null;
  const lines = [];
  if (r.off > 0) lines.push(`off on ${r.off} of ${reporting} nodes`);
  if (r.unavailable > 0) lines.push(`not in the build on ${r.unavailable} of ${reporting} nodes`);
  return lines.length ? { lines, names: r.off_instances.join(", ") } : null;
}

function span(secs: number): string {
  const abs = Math.abs(secs);
  return abs < 3600 ? `${Math.max(1, Math.floor(abs / 60))} min` : abs < 86400 ? `${Math.floor(abs / 3600)} h` : `${Math.floor(abs / 86400)} d`;
}

/** The `features:` section of the current config; `null` when the text cannot be read. */
function configFlags(text: string): Record<string, boolean> | null {
  try {
    const features = (parse(text) as { features?: Record<string, unknown> } | null)?.features ?? {};
    return Object.fromEntries(Object.entries(features).filter(([, v]) => typeof v === "boolean")) as Record<string, boolean>;
  } catch {
    return null;
  }
}

/**
 * The controller answers for its own flags with every source applied. For the proxy flags it
 * knows the default and any override; the config's `features:` value sits between them, so it
 * is added here unless an override already decides.
 */
function merge(flags: FeatureFlag[], proxy: FeatureFlag[], config: Record<string, boolean> | null): FeatureFlag[] {
  const withConfig = proxy.map((f) => {
    const v = config?.[f.name];
    return f.source === "default" && f.available && v !== undefined ? { ...f, value: v, source: "config" as const } : f;
  });
  return [...withConfig, ...flags];
}

function OverrideDialog({ flag, onClose, onDone }: { flag: FeatureFlag; onClose: () => void; onDone: () => void }) {
  const turnOn = !flag.value;
  const needsExpiry = flag.protection && !turnOn;
  const [reason, setReason] = useState("");
  const [ttl, setTtl] = useState(needsExpiry ? "3600" : "never");
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const verb = `Turn ${turnOn ? "on" : "off"} ${flag.name}`;

  async function go() {
    if (!reason.trim()) return setProblem("Give a reason: it is recorded with your name.");
    setProblem(null);
    setBusy(true);
    try {
      await putFeatureOverride(flag.name, turnOn, reason.trim(), ttl === "never" ? undefined : Number(ttl));
      onDone();
    } catch (err) {
      setProblem(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()} title={verb} description={`An override beats the config until it is cleared${ttl === "never" ? "" : " or ends"}.`}>
      <div className="flex flex-col gap-3 text-sm">
        <p>
          <span className="font-semibold">If it is off: </span>
          <span>{flag.risk}</span>
        </p>
        <p>
          <span className="font-semibold">Open connections: </span>
          <span>{flag.impact}</span>
        </p>
        <p className="text-ink-muted">{flag.apply === "live" ? "Takes effect within seconds, on every node that reads it." : "Takes effect when the process restarts."}</p>
        {needsExpiry && <p>Turning this off removes protection, so it comes back on by itself.</p>}
        <label className="flex flex-col gap-1">
          <span className="font-semibold">Reason</span>
          <Textarea rows={2} value={reason} onChange={(e) => setReason(e.target.value)} />
        </label>
        <label className="flex flex-col gap-1">
          <span className="font-semibold">Back on after</span>
          <Select value={ttl} onChange={(e) => setTtl(e.target.value)}>
            {!needsExpiry && <option value="never">never (until cleared)</option>}
            {TTLS.map(([label, secs]) => (
              <option key={secs} value={secs}>
                {label}
              </option>
            ))}
          </Select>
        </label>
        {problem && (
          <p role="alert" className="text-bad">
            {problem}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="danger" loading={busy} onClick={go}>
            {verb}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

// Configure > Features: every feature flag with its effective value and where that comes
// from, and the emergency override (docs/05-configuration.md, "Feature flags").
export function FeaturesPage() {
  const admin = useCan("admin");
    const [rows, setRows] = useState<FeatureFlag[] | null>(null);
  const [overrides, setOverrides] = useState<Record<string, FeatureOverride>>({});
  const [configRead, setConfigRead] = useState(true);
  const [overridesRead, setOverridesRead] = useState(true);
  const [error, setError] = useState<Error | null>(null);
  const [fleet, setFleet] = useState<{ reporting: number; byName: Record<string, FlagRollup> } | null>(null);
  const [clearing, setClearing] = useState<string | null>(null);
  const [editing, setEditing] = useState<FeatureFlag | null>(null);

  const refresh = useCallback(async () => {
    const [f, o, c, n] = await Promise.allSettled([getFeatures(), listFeatureOverrides(), getCurrentConfig(), getFleetFeatures()]);
    if (f.status === "rejected") {
      setError(f.reason instanceof Error ? f.reason : new Error(String(f.reason)));
      return;
    }
    setError(null);
    const config = c.status === "fulfilled" ? configFlags(c.value.text) : null;
    setConfigRead(config !== null);
    setOverridesRead(o.status === "fulfilled");
    const now = Math.floor(Date.now() / 1000);
    setOverrides(Object.fromEntries((o.status === "fulfilled" ? o.value.items : []).filter((x) => !x.expires_at || x.expires_at > now).map((x) => [x.name, x])));
    setFleet(n.status === "fulfilled" ? { reporting: n.value.reporting, byName: Object.fromEntries(n.value.flags.map((x) => [x.name, x])) } : null);
    setRows(merge(f.value.flags, f.value.proxy_flags, config));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  async function clear(flag: FeatureFlag) {
    setClearing(null);
    try {
      await clearFeatureOverride(flag.name);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err : new Error(String(err)));
    }
  }

  const unwired = rows?.filter((f) => !f.wired && f.available) ?? [];
  const visible = rows?.filter((f) => f.wired) ?? [];
  const reduced = visible.filter((f) => f.protection && !f.value);

  return (
    <div className="max-w-5xl">
      <PageHeader title="Features" summary="Switches for single capabilities. The config sets them; an override is the emergency exception on top." />
      {error && <ErrorState onRetry={refresh}>{error.message}</ErrorState>}
      {rows === null && !error && <Loading />}
      {reduced.length > 0 && (
        <AlertBar>
          Protection is reduced: {reduced.map((f) => f.name).join(", ")} {reduced.length === 1 ? "is" : "are"} off
          {reduced.every((f) => overrides[f.name]?.expires_at) && ", and the override ends by itself"}.
        </AlertBar>
      )}
      {!configRead && <p className="mb-3 text-sm text-ink-muted">The config could not be read, so config values are not shown; flags set there look like their default.</p>}
      {!overridesRead && <p className="mb-3 text-sm text-ink-muted">The overrides could not be read, so their reasons and end times are not shown.</p>}
      {rows && (
        <>
          {visible.length === 0 ? (
            <p className="text-sm text-ink-muted">No flag is enforced by this build yet.</p>
          ) : (
            <Table>
              <THead>
                <TR>
                  <TH>flag</TH>
                  <TH>value</TH>
                  <TH>from</TH>
                  <TH>if turned off</TH>
                  <TH>
                    <span className="sr-only">actions</span>
                  </TH>
                </TR>
              </THead>
              <TBody>
                {visible.map((f) => {
                  const o = f.source === "override" ? overrides[f.name] : undefined;
                  const note = fleetNote(fleet?.byName[f.name], fleet?.reporting ?? 0);
                  const canFlip = f.available && !(f.override === "off_only" && !f.value);
                  return (
                    <TR key={f.name} className="align-top">
                      <TD className="text-ink">
                        {LABELS[f.name] && <div>{LABELS[f.name]}</div>}
                        <code className="font-mono text-xs text-ink-muted">{f.name}</code>
                      </TD>
                      <TD>
                        <Badge tone={f.value ? "good" : f.protection ? "bad" : "neutral"}>{f.value ? "on" : "off"}</Badge>
                        {note && (
                          <div className="mt-1 text-xs text-ink-muted" title={note.names}>
                            {note.lines.map((l) => (
                              <p key={l}>{l}</p>
                            ))}
                            {note.names && <p>{note.names}</p>}
                          </div>
                        )}
                      </TD>
                      <TD className="text-ink-muted">{SOURCE_LABEL[f.source]}</TD>
                      <TD className="max-w-md text-ink-muted">
                        <p>{f.risk}</p>
                        {o && (
                          <p className="mt-1 text-xs">
                            “{o.reason}”{o.actor && ` · ${o.actor}`}
                            {o.expires_at && ` · ends in ${span(o.expires_at - Math.floor(Date.now() / 1000))}`}
                          </p>
                        )}
                      </TD>
                      <TD className="whitespace-nowrap text-right">
                        {f.source === "override" && clearing === f.name ? (
                          <span className="inline-flex items-center gap-2">
                            <span className="text-xs text-ink-muted">Back to the config value or default?</span>
                            <Button size="sm" variant="danger" aria-label={`Yes, clear ${f.name}`} onClick={() => clear(f)}>
                              Yes, clear
                            </Button>
                            <Button size="sm" variant="ghost" aria-label={`Keep ${f.name} override`} onClick={() => setClearing(null)}>
                              Keep
                            </Button>
                          </span>
                        ) : (
                          f.source === "override" && (
                            <Button size="sm" disabled={!admin} aria-label={`Clear override of ${f.name}`} onClick={() => setClearing(f.name)}>
                              Clear override
                            </Button>
                          )
                        )}{" "}
                        {canFlip && (
                          <Button size="sm" disabled={!admin} aria-label={`Override ${f.name}`} onClick={() => setEditing(f)}>
                            Override
                          </Button>
                        )}
                      </TD>
                    </TR>
                  );
                })}
              </TBody>
            </Table>
          )}
          {unwired.length > 0 && (
            <p className="mt-4 text-sm text-ink-muted">
              In the registry but not enforced by this build yet, so they are hidden: <span className="font-mono">{unwired.map((f) => f.name).join(", ")}</span>.
            </p>
          )}
        </>
      )}
      {editing && (
        <OverrideDialog
          flag={editing}
          onClose={() => setEditing(null)}
          onDone={() => {
            setEditing(null);
            refresh();
          }}
        />
      )}
    </div>
  );
}
