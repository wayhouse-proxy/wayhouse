import { useEffect, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { ApiError, deletePlugin, listPluginRoutes, listPlugins, setPluginEnabled } from "../api";
import type { PluginInstall, PluginRouteOverlay } from "../types";
import { bytes, CapabilityList } from "../lib/plugins/capabilities";
import { Badge } from "../components/ui/Badge";
import { useCan } from "../lib/session";
import { PageHeader } from "../components/shell/Page";
import { EmptyState, ErrorState, Loading } from "../components/shell/States";
import { Button } from "../components/ui/Button";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { useConfirm } from "../components/ui/ConfirmDialog";

function ago(unixSecs: number): string {
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - unixSecs);
  if (secs < 60) return "just now";
  if (secs < 3600) return `${Math.floor(secs / 60)} min ago`;
  if (secs < 86_400) return `${Math.floor(secs / 3600)} h ago`;
  return `${Math.floor(secs / 86_400)} d ago`;
}

function errorText(err: unknown): string {
  return err instanceof ApiError ? err.message : String(err);
}

// Plugins on the controller (docs/plugins.md): the installs and their port routes. Installing and
// editing one are their own pages; registry install is a later slice.
export function PluginsPage() {
  const canWrite = useCan("admin");
  const [plugins, setPlugins] = useState<PluginInstall[] | null>(null);
  const [overlay, setOverlay] = useState<PluginRouteOverlay | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The wizard hands over "installed X" when it comes back here.
  const [notice, setNotice] = useState<string | null>((useLocation().state as { notice?: string } | null)?.notice ?? null);
  const [busy, setBusy] = useState(false);
  const { confirm, dialog } = useConfirm();

  async function refresh() {
    try {
      setPlugins(await listPlugins());
      setError(null);
    } catch (err) {
      setPlugins(null);
      setError(errorText(err));
    }
    // The overlay is extra: a controller without it (or without plugins) just shows no ports.
    try {
      setOverlay(await listPluginRoutes());
    } catch {
      setOverlay(null);
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  async function toggle(p: PluginInstall) {
    if (p.enabled) {
      const ok = await confirm({
        title: `Disable ${p.name}?`,
        description: "The plugin stops running until you enable it again.",
        confirmLabel: "Disable",
      });
      if (!ok) return;
    }
    setBusy(true);
    try {
      await setPluginEnabled(p.id, !p.enabled);
      setNotice(`${p.enabled ? "disabled" : "enabled"} ${p.name}`);
    } catch (err) {
      setNotice(`${p.enabled ? "disable" : "enable"} ${p.name} failed: ${errorText(err)}`);
    } finally {
      setBusy(false);
    }
    await refresh();
  }

  async function remove(p: PluginInstall) {
    const ok = await confirm({
      title: `Delete ${p.name}?`,
      description:
        "Removes the install and its approval. The uploaded module is kept only while another install uses it.",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    setBusy(true);
    try {
      await deletePlugin(p.id);
      setNotice(`deleted ${p.name}`);
    } catch (err) {
      setNotice(`delete ${p.name} failed: ${errorText(err)}`);
    } finally {
      setBusy(false);
    }
    await refresh();
  }

  return (
    <div className="max-w-4xl">
      <PageHeader
        title="Plugins"
        summary="Modules that add integrations. A plugin runs only with the capabilities you approve."
        actions={
          canWrite && (
            <Link
              to="/plugins/new"
              className="inline-flex min-h-(--control-height) items-center rounded-sm border border-primary bg-primary px-[11px] text-sm font-semibold text-primary-ink hover:opacity-90"
            >
              Install plugin
            </Link>
          )
        }
      />

      {notice && (
        <p role="status" className="mb-4 text-sm text-ink-muted">{notice}</p>
      )}

      <div>
        {error && <ErrorState>{error}</ErrorState>}
        {!error && plugins === null && <Loading />}
        {!error && plugins?.length === 0 && (
          <EmptyState>No plugins installed yet. Install a module to add one.</EmptyState>
        )}
        {!error && plugins && plugins.length > 0 && (
          <Table>
            <THead>
              <TR>
                <TH>name</TH>
                <TH>approved</TH>
                <TH>module</TH>
                <TH>status</TH>
                <TH />
              </TR>
            </THead>
            <TBody>
              {plugins.map((p) => (
                <TR key={p.id} className="align-top">
                  <TD>
                    <Link to={`/plugins/${p.id}`} className="font-mono text-ink underline">
                      {p.name}
                    </Link>
                    <div className="text-xs text-ink-muted">
                      installed {ago(p.created_at)}
                      {p.created_by ? ` by ${p.created_by}` : ""}
                    </div>
                  </TD>
                  <TD>
                    <CapabilityList caps={p.approved} />
                  </TD>
                  <TD>
                    <div className="font-mono text-xs text-ink-muted" title={p.sha256}>
                      {p.sha256.slice(0, 12)}…
                    </div>
                    <div className="text-xs text-ink-muted">{bytes(p.size)}</div>
                  </TD>
                  <TD>
                    {p.enabled ? <Badge tone="good">enabled</Badge> : <Badge tone="neutral">disabled</Badge>}
                  </TD>
                  <TD className="text-right whitespace-nowrap">
                    <Button
                      className="mr-2"
                      disabled={busy || !canWrite}
                      aria-label={`${p.enabled ? "Disable" : "Enable"} ${p.name}`}
                      onClick={() => toggle(p)}
                    >
                      {p.enabled ? "Disable" : "Enable"}
                    </Button>
                    <Button
                      variant="danger"
                      disabled={busy || !canWrite}
                      aria-label={`Delete ${p.name}`}
                      onClick={() => remove(p)}
                    >
                      Delete
                    </Button>
                  </TD>
                </TR>
              ))}
            </TBody>
          </Table>
        )}
      </div>

      {overlay && <PortRoutes overlay={overlay} />}

      <p className="mt-4 text-sm text-ink-muted">
        A plugin can still misuse what you approve: it can read and act on whatever its capabilities
        reach. Approve only modules you trust.
      </p>

      {dialog}
    </div>
  );
}

const PROTO_WORDS = { udp: "udp", tcp: "tcp", both: "udp and tcp" } as const;

/** The entry ports plugins hold, port first: players connect to the wayhouse name at that port. */
function PortRoutes({ overlay }: { overlay: PluginRouteOverlay }) {
  const ports = overlay.port_routes ?? [];
  const exhausted = overlay.exhausted ?? [];
  if (ports.length === 0 && exhausted.length === 0) return null;
  return (
    <section className="mt-6">
      <h2 className="mb-2 text-lg font-semibold">Port routes</h2>
      {exhausted.length > 0 && (
        <p className="mb-2 text-sm text-ink-muted">
          No free entry port for {exhausted.map((e) => e.key).join(", ")} ({exhausted[0].plugin}
          {exhausted.length > 1 ? " and others" : ""}). Approve a wider port range or remove servers.
        </p>
      )}
      {ports.length > 0 && (
        <div>
          <Table>
            <THead>
              <TR>
                <TH>entry port</TH>
                <TH>key</TH>
                <TH>protocol</TH>
                <TH>backend</TH>
                <TH>plugin</TH>
              </TR>
            </THead>
            <TBody>
              {[...ports]
                .sort((a, b) => a.port - b.port)
                .map((r) => (
                  <TR key={`${r.owner}/${r.key}`}>
                    <TD className="font-mono text-ink">{r.port}</TD>
                    <TD className="font-mono text-xs text-ink-muted">{r.key}</TD>
                    <TD className="text-ink-muted">{PROTO_WORDS[r.proto]}</TD>
                    <TD className="font-mono text-xs text-ink-muted">{r.backend}</TD>
                    <TD className="text-ink-muted">{r.plugin}</TD>
                  </TR>
                ))}
            </TBody>
          </Table>
        </div>
      )}
    </section>
  );
}
