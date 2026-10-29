import { useCallback, useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import {
  ApiError,
  deletePluginSecret,
  enablePluginWebhook,
  getPlugin,
  getPluginStatus,
  listPluginSecrets,
  putPluginConfig,
  putPluginSecret,
  revokePluginWebhook,
} from "../api";
import type { PluginCapabilities, PluginDetail, PluginSecretSlot, PluginStatus } from "../types";
import { ApprovalDialog } from "../components/plugins/ApprovalDialog";
import { SchemaForm } from "../components/form/SchemaForm";
import { PageHeader, SaveBar } from "../components/shell/Page";
import { EmptyState, ErrorState, Loading } from "../components/shell/States";
import { Badge } from "../components/ui/Badge";
import { Button, Input } from "../components/ui/Button";
import { Dialog } from "../components/ui/Dialog";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "../components/ui/Tabs";
import { CapabilityList } from "../lib/plugins/capabilities";
import { optionsForm } from "../lib/plugins/options";
import { useCan } from "../lib/session";

const errorText = (err: unknown) => (err instanceof ApiError ? err.message : String(err));

function ago(unixSecs: number): string {
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - unixSecs);
  if (secs < 60) return "just now";
  if (secs < 3600) return `${Math.floor(secs / 60)} min ago`;
  if (secs < 86_400) return `${Math.floor(secs / 3600)} h ago`;
  return `${Math.floor(secs / 86_400)} d ago`;
}

// One install: its options (with re-approval when they move the capabilities), secrets, webhook and status.
export function PluginDetailPage() {
  const { id = "" } = useParams();
  const [plugin, setPlugin] = useState<PluginDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    try {
      setPlugin(await getPlugin(id));
      setError(null);
    } catch (err) {
      setError(errorText(err));
    }
  }, [id]);
  useEffect(() => {
    load();
  }, [load]);

  if (error) return <ErrorState onRetry={load}>{error}</ErrorState>;
  if (!plugin) return <Loading />;
  const wantsWebhook = !!plugin.approved.triggers?.on_webhook;
  const hasSecrets = (plugin.approved.secrets?.length ?? 0) > 0;
  return (
    <div className="max-w-3xl">
      <PageHeader
        title={plugin.name}
        summary={
          <>
            {plugin.enabled ? <Badge tone="good">enabled</Badge> : <Badge tone="neutral">disabled</Badge>}{" "}
            <Link to="/plugins" className="underline">
              All plugins
            </Link>
          </>
        }
      />
      <Tabs defaultValue="options">
        <TabsList>
          <TabsTrigger value="options">Options</TabsTrigger>
          {hasSecrets && <TabsTrigger value="secrets">Secrets</TabsTrigger>}
          {wantsWebhook && <TabsTrigger value="webhook">Webhook</TabsTrigger>}
          <TabsTrigger value="status">Status and log</TabsTrigger>
        </TabsList>
        <TabsContent value="options" className="pt-4">
          <OptionsTab plugin={plugin} onSaved={load} />
        </TabsContent>
        {hasSecrets && (
          <TabsContent value="secrets" className="pt-4">
            <SecretsTab plugin={plugin} />
          </TabsContent>
        )}
        {wantsWebhook && (
          <TabsContent value="webhook" className="pt-4">
            <WebhookTab plugin={plugin} onChanged={load} />
          </TabsContent>
        )}
        <TabsContent value="status" className="pt-4">
          <StatusTab id={plugin.id} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

function OptionsTab({ plugin, onSaved }: { plugin: PluginDetail; onSaved: () => void }) {
  const canWrite = useCan("admin");
  const form = optionsForm(plugin.manifest);
  const saved = (plugin.config ?? {}) as Record<string, unknown>;
  const [config, setConfig] = useState(saved);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [needs, setNeeds] = useState<{ approved: PluginCapabilities; effective: PluginCapabilities } | null>(null);
  const changes = JSON.stringify(config) === JSON.stringify(saved) ? 0 : 1;

  async function save(approved?: PluginCapabilities) {
    setBusy(true);
    setError(null);
    try {
      await putPluginConfig(plugin.id, config, approved);
      setNeeds(null);
      setNotice("Options saved.");
      onSaved();
    } catch (err) {
      // The new options move what is approved: show both sets and let the operator decide.
      if (err instanceof ApiError && err.status === 409 && err.body?.effective) {
        setNeeds({ approved: err.body.approved, effective: err.body.effective });
      } else setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }

  if (!form.hasFields)
    return (
      <>
        <EmptyState>This module has no options.</EmptyState>
        <h2 className="mt-2 mb-1 text-sm font-semibold">Approved capabilities</h2>
        <CapabilityList caps={plugin.approved} />
      </>
    );
  return (
    <>
      <SchemaForm schema={form.schema} layout={form.layout} value={config} saved={saved} onChange={(v) => setConfig(v as Record<string, unknown>)} />
      {form.secrets.length > 0 && (
        <p className="mt-2 text-sm text-ink-muted">{form.secrets.join(", ")}: set under Secrets, not here.</p>
      )}
      <h2 className="mt-4 mb-1 text-sm font-semibold">Approved capabilities</h2>
      <CapabilityList caps={plugin.approved} />
      {notice && !error && <p role="status" className="mt-2 text-sm text-ink-muted">{notice}</p>}
      {error && <ErrorState>{error}</ErrorState>}
      {canWrite && <SaveBar changes={changes} applying={busy && !needs} onDiscard={() => setConfig(saved)} onApply={() => save()} />}
      {needs && (
        <ApprovalDialog
          open
          title="These options change what the plugin may do"
          intro="Saving applies the new options only together with a new approval."
          before={needs.approved}
          caps={needs.effective}
          confirmLabel="Approve and save"
          busy={busy}
          error={error}
          onConfirm={() => save(needs.effective)}
          onCancel={() => setNeeds(null)}
        />
      )}
    </>
  );
}

function SecretsTab({ plugin }: { plugin: PluginDetail }) {
  const canWrite = useCan("admin");
  const [slots, setSlots] = useState<PluginSecretSlot[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [setting, setSetting] = useState<PluginSecretSlot | null>(null);
  const load = useCallback(async () => {
    try {
      setSlots(await listPluginSecrets(plugin.id));
      setError(null);
    } catch (err) {
      setError(errorText(err));
    }
  }, [plugin.id]);
  useEffect(() => {
    load();
  }, [load]);

  async function clear(s: PluginSecretSlot) {
    try {
      await deletePluginSecret(plugin.id, s.slot);
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  }

  if (!slots && !error) return <Loading />;
  return (
    <>
      {error && <ErrorState onRetry={load}>{error}</ErrorState>}
      {slots && (
        <Table>
          <THead>
            <TR>
              <TH>slot</TH>
              <TH>sent only to</TH>
              <TH>state</TH>
              <TH />
            </TR>
          </THead>
          <TBody>
            {slots.map((s) => (
              <TR key={s.slot} className="align-top">
                <TD>
                  <div className="font-mono text-ink">{s.slot}</div>
                  <div className="text-xs text-ink-muted">{s.description}</div>
                </TD>
                <TD className="font-mono text-xs text-ink-muted">{s.bound_hosts.join(", ")}</TD>
                <TD>
                  {s.status === "ok" ? (
                    <Badge tone="good">set</Badge>
                  ) : s.status === "unset" ? (
                    <Badge tone="neutral">not set</Badge>
                  ) : (
                    <span className="text-sm text-bad">{s.status}</span>
                  )}
                </TD>
                <TD className="text-right whitespace-nowrap">
                  <Button className="mr-2" disabled={!canWrite} aria-label={`${s.set ? "Replace" : "Set"} ${s.slot}`} onClick={() => setSetting(s)}>
                    {s.set ? "Replace" : "Set"}
                  </Button>
                  {s.set && (
                    <Button variant="danger" disabled={!canWrite} aria-label={`Clear ${s.slot}`} onClick={() => clear(s)}>
                      Clear
                    </Button>
                  )}
                </TD>
              </TR>
            ))}
          </TBody>
        </Table>
      )}
      <p className="mt-2 text-sm text-ink-muted">Values are write-only: once saved they cannot be read back, here or anywhere.</p>
      {setting && (
        <SecretDialog
          slot={setting}
          onSave={(value) => putPluginSecret(plugin.id, setting.slot, value)}
          onClose={(saved) => {
            setSetting(null);
            if (saved) load();
          }}
        />
      )}
    </>
  );
}

function SecretDialog({ slot, onSave, onClose }: { slot: PluginSecretSlot; onSave: (v: string) => Promise<void>; onClose: (saved: boolean) => void }) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      await onSave(value);
      setValue("");
      onClose(true);
    } catch (err) {
      setError(errorText(err));
      setBusy(false);
    }
  }
  return (
    <Dialog open onOpenChange={(o) => !o && onClose(false)} title={`${slot.set ? "Replace" : "Set"} ${slot.slot}`} description={slot.description}>
      <form className="flex flex-col gap-3" onSubmit={submit}>
        <label className="flex flex-col gap-1 text-sm text-ink-muted">
          Value
          <Input type="password" autoComplete="off" mono value={value} onChange={(e) => setValue(e.target.value)} />
        </label>
        <p className="text-sm text-ink-muted">Between 16 and 4096 bytes. It is sent to the controller and never shown again.</p>
        {error && <ErrorState>{error}</ErrorState>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onClose(false)}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" loading={busy} disabled={value === ""}>
            Save secret
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

function WebhookTab({ plugin, onChanged }: { plugin: PluginDetail; onChanged: () => void }) {
  const canWrite = useCan("admin");
  const [token, setToken] = useState<{ token: string; path: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const on = plugin.webhook != null;

  async function run(action: () => Promise<unknown>) {
    setError(null);
    try {
      await action();
      onChanged();
    } catch (err) {
      setError(errorText(err));
    }
  }

  return (
    <>
      <p className="mb-3 text-sm text-ink-muted">
        {on ? "The webhook is enabled." : "The webhook is off."} Callers send the token as a bearer token to the path shown when it is created.
      </p>
      {token && (
        <div role="status" className="mb-3 border border-line bg-surface p-3 text-sm">
          <p className="font-medium text-ink">Copy the token now; it is shown only once.</p>
          <p className="mt-1 break-all font-mono text-xs">{token.token}</p>
          <p className="mt-1 text-ink-muted">
            POST to <span className="font-mono text-xs">{token.path}</span>
          </p>
          <Button className="mt-2" size="sm" onClick={() => navigator.clipboard?.writeText(token.token)}>
            Copy token
          </Button>
        </div>
      )}
      {error && <ErrorState>{error}</ErrorState>}
      <div className="flex gap-2">
        <Button disabled={!canWrite} onClick={() => run(async () => setToken(await enablePluginWebhook(plugin.id)))}>
          {on ? "Rotate token" : "Enable webhook"}
        </Button>
        {on && (
          <Button
            variant="danger"
            disabled={!canWrite}
            onClick={() =>
              run(async () => {
                await revokePluginWebhook(plugin.id);
                setToken(null);
              })
            }
          >
            Revoke webhook
          </Button>
        )}
      </div>
    </>
  );
}

function StatusTab({ id }: { id: string }) {
  const [status, setStatus] = useState<PluginStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    try {
      setStatus(await getPluginStatus(id));
      setError(null);
    } catch (err) {
      setError(errorText(err));
    }
  }, [id]);
  useEffect(() => {
    load();
  }, [load]);

  if (error) return <ErrorState onRetry={load}>{error}</ErrorState>;
  if (!status) return <Loading />;
  const rows: [string, React.ReactNode][] = [
    ["Ticks", status.ticks],
    ["Last tick", status.last_tick_unix ? `${ago(status.last_tick_unix)}, ${status.last_ok ? "ok" : "failed"}` : "not yet"],
    ["Consecutive failures", status.consecutive_failures],
    ["Webhook and event calls", `${status.hook_calls ?? 0}, ${status.hook_failures ?? 0} failed, ${status.hooks_dropped ?? 0} dropped`],
  ];
  return (
    <>
      {status.held && (
        <p role="alert" className="mb-3 text-sm text-bad">
          Not running on this node: {status.held}
        </p>
      )}
      {status.last_error && <p className="mb-3 text-sm text-bad">Last error: {status.last_error}</p>}
      <dl className="mb-4 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-ink-muted">{k}</dt>
            <dd>{v}</dd>
          </div>
        ))}
      </dl>
      <div className="mb-1 flex items-center justify-between">
        <h2 className="text-sm font-semibold">Log (last {status.logs.length} lines, this node)</h2>
        <Button size="sm" onClick={load}>
          Refresh
        </Button>
      </div>
      {status.logs.length === 0 ? (
        <EmptyState>No log lines yet.</EmptyState>
      ) : (
        <ul className="max-h-80 overflow-auto border border-line bg-surface-sunken p-2 font-mono text-xs">
          {status.logs.map((l, i) => (
            <li key={i}>
              <span className="text-ink-muted">{l.level}</span> {l.msg}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}
