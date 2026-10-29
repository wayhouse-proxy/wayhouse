import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { useEffect, useState } from "react";
import { ApiError, diffRevision, listRevisions, rollbackTo } from "../api";
import type { RevisionSummary } from "../types";
import { Badge } from "../components/ui/Badge";
import { Button } from "../components/ui/Button";
import { PageHeader } from "../components/shell/Page";
import { EmptyState, Loading } from "../components/shell/States";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { useCan } from "../lib/session";

export function ConfigHistoryPage() {
  const [revisions, setRevisions] = useState<RevisionSummary[]>([]);
  const [diff, setDiff] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const canWrite = useCan("admin");
  const { confirm, dialog } = useConfirm();

  async function refresh() {
    try {
      setRevisions(await listRevisions());
    } catch (err) {
      setNotice(err instanceof ApiError ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function showDiff(rev: number) {
    try {
      setDiff(await diffRevision(rev));
    } catch (err) {
      setNotice(err instanceof ApiError ? err.message : String(err));
    }
  }

  async function doRollback(rev: number) {
    const ok = await confirm({
      title: `Roll back to revision ${rev}?`,
      description: `Re-submits revision ${rev} as a new revision; every subscribed proxy picks it up. History is not rewritten.`,
      confirmLabel: "Roll back",
    });
    if (!ok) return;
    setBusy(true);
    try {
      const result = await rollbackTo(rev);
      setNotice(`rolled back to revision ${rev} as new revision ${result.revision}`);
      await refresh();
    } catch (err) {
      setNotice(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  if (loading) return <Loading />;

  return (
    <div className="max-w-3xl">
      <PageHeader title="Config history" summary="Every submitted configuration is a revision. Compare one with the current revision or roll back to it." />

      {notice && (
        <p role="status" className="mb-4 text-sm text-ink-muted">{notice}</p>
      )}

      <div>
        {revisions.length === 0 ? (
          <EmptyState>No revisions submitted yet.</EmptyState>
        ) : (
          <Table>
            <THead>
              <TR>
                <TH>revision</TH>
                <TH>size</TH>
                <TH></TH>
                <TH></TH>
              </TR>
            </THead>
            <TBody>
              {revisions.map((r) => (
                <TR key={r.revision}>
                  <TD className="font-mono text-ink">{r.revision}</TD>
                  <TD className="text-ink-muted">{r.size_bytes} B</TD>
                  <TD>{r.current && <Badge tone="good">current</Badge>}</TD>
                  <TD className="text-right">
                    <Button variant="ghost" size="sm" onClick={() => showDiff(r.revision)}>
                      diff vs. current
                    </Button>
                    {!r.current && (
                      <Button variant="danger" size="sm" onClick={() => doRollback(r.revision)}
                      disabled={busy || !canWrite}
                      title={canWrite ? undefined : "Rolling back needs the admin role"}
                    >
                        roll back to this
                      </Button>
                    )}
                  </TD>
                </TR>
              ))}
            </TBody>
          </Table>
        )}
      </div>

      {diff !== null && (
        <section className="mt-4">
          <h2 className="mb-2 text-lg font-semibold">Diff</h2>
          <pre className="overflow-x-auto border border-line bg-surface-sunken p-3 font-mono text-xs">{diff}</pre>
        </section>
      )}
      {dialog}
    </div>
  );
}
