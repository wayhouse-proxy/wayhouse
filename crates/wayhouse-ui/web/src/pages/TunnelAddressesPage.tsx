import { useEffect, useState } from "react";
import { ApiError, getTunnelAddresses, releaseTunnelAddress } from "../api";
import type { TunnelAddresses } from "../types";
import { Badge } from "../components/ui/Badge";
import { PageHeader, AlertBar } from "../components/shell/Page";
import { EmptyState, ErrorState, Loading } from "../components/shell/States";
import { Button } from "../components/ui/Button";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { useConfirm } from "../components/ui/ConfirmDialog";

/** "2 min ago" / "3 h ago" / "20 d ago" from unix seconds; the exact time is the cell's tooltip. */
function ago(unixSecs: number): string {
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - unixSecs);
  if (secs < 60) return "just now";
  if (secs < 3600) return `${Math.floor(secs / 60)} min ago`;
  if (secs < 86_400) return `${Math.floor(secs / 3600)} h ago`;
  return `${Math.floor(secs / 86_400)} d ago`;
}

function exact(unixSecs: number): string {
  return new Date(unixSecs * 1000).toISOString();
}

// Read-only view of wayhouse-controller's tunnel address book (`GET /tunnel/addresses`,
// docs/11 "Address authority"). Releasing an address is the registry DELETE,
// proxied by wayhouse-ui behind a confirmation.
export function TunnelAddressesPage() {
  const [table, setTable] = useState<TunnelAddresses | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const { confirm, dialog } = useConfirm();

  async function refresh() {
    setLoading(true);
    try {
      setTable(await getTunnelAddresses());
      setError(null);
    } catch (err) {
      setTable(null);
      setError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  async function handleRelease(role: "origin" | "proxy", name: string, address: string) {
    const ok = await confirm({
      title: `Release ${address}?`,
      description: `Removes ${role} ${name}'s registration and frees its address for reuse. A ${role} that is still running re-registers and may be given a different address.`,
      confirmLabel: "Release address",
    });
    if (!ok) return;
    setBusy(true);
    try {
      const res = await releaseTunnelAddress(role, name);
      setNotice(`released ${res.released ?? address} from ${name}`);
    } catch (err) {
      setNotice(`release ${name} failed: ${err instanceof ApiError ? err.message : err}`);
    } finally {
      setBusy(false);
    }
    await refresh();
  }

  useEffect(() => {
    refresh();
  }, []);

  const staleCount = table?.entries.filter((e) => e.stale).length ?? 0;

  return (
    <div className="max-w-4xl">
      <PageHeader
        title="Tunnel addresses"
        summary="Addresses the controller assigned to origins and proxies on the WireGuard network."
        actions={
          <Button onClick={refresh} loading={loading}>
            Refresh
          </Button>
        }
      />

      {notice && (
        <p role="status" className="mb-4 text-sm text-ink-muted">{notice}</p>
      )}

      {error && (
        <ErrorState>{error}</ErrorState>
      )}

      {table === null && loading && !error && <Loading />}

      {table && (
        <>
          <p className="mb-4 text-sm text-ink-muted">
            {table.network ? (
              <>
                Network <span className="font-mono text-ink">{table.network}</span>
                {" · "}
                {table.capacity !== null
                  ? `${table.allocated} of ${table.capacity} allocated`
                  : `${table.allocated} allocated`}
              </>
            ) : (
              "The controller runs with no --tunnel-network, so only pinned addresses are listed."
            )}
            {staleCount > 0 && ` · ${staleCount} stale`}
          </p>

          {staleCount > 0 && (
            <AlertBar>
              A stale owner has not re-registered for longer than the controller's{" "}
              <span className="font-mono">--tunnel-stale-after</span>. Nothing is freed automatically: release
              its address here once you know the owner is gone.
            </AlertBar>
          )}

          <div>
            {table.entries.length === 0 ? (
              <EmptyState>No tunnel addresses assigned yet.</EmptyState>
            ) : (
              <Table>
                <THead>
                  <TR>
                    <TH>address</TH>
                    <TH>role</TH>
                    <TH>name</TH>
                    <TH>first seen</TH>
                    <TH>last seen</TH>
                    <TH>status</TH>
                    <TH />
                  </TR>
                </THead>
                <TBody>
                  {table.entries.map((e) => (
                    <TR key={`${e.role}/${e.name}`}>
                      <TD className="font-mono text-ink">{e.address}</TD>
                      <TD className="text-ink-muted">{e.role}</TD>
                      <TD className="font-mono text-ink">{e.name}</TD>
                      <TD className="text-ink-muted" title={exact(e.first_seen)}>
                        {ago(e.first_seen)}
                      </TD>
                      <TD className="text-ink-muted" title={exact(e.last_seen)}>
                        {ago(e.last_seen)}
                      </TD>
                      <TD>
                        {e.stale ? <Badge tone="warn">stale</Badge> : <Badge tone="good">active</Badge>}
                      </TD>
                      <TD className="text-right">
                        <Button
                          variant="danger"
                          disabled={busy}
                          aria-label={`Release ${e.name}`}
                          onClick={() => handleRelease(e.role, e.name, e.address)}
                        >
                          Release
                        </Button>
                      </TD>
                    </TR>
                  ))}
                </TBody>
              </Table>
            )}
          </div>
        </>
      )}
      {dialog}
    </div>
  );
}
