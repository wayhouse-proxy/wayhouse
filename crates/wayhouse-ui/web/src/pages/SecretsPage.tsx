import { useCallback, useEffect, useState } from "react";
import { ApiError, deleteClusterSecret, listClusterSecrets, putClusterSecret } from "../api";
import type { ClusterSecret } from "../types";
import { PageHeader } from "../components/shell/Page";
import { ErrorState, Loading } from "../components/shell/States";
import { Button } from "../components/ui/Button";
import { Dialog } from "../components/ui/Dialog";
import { Input } from "../components/ui/Input";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { validSecretName } from "../lib/secretName";
import { useCan } from "../lib/session";

const errorText = (err: unknown) => (err instanceof ApiError ? err.message : String(err));

const when = (s: number) => new Date(s * 1000).toISOString().slice(0, 16).replace("T", " ") + " UTC";

// Cluster secrets: the values a config refers to as `{ secret: name }`. Write-only.
export function SecretsPage() {
  const canWrite = useCan("admin");
  const [items, setItems] = useState<ClusterSecret[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [confirming, setConfirming] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setItems(await listClusterSecrets());
      setError(null);
    } catch (err) {
      setError(errorText(err));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  async function remove(name: string) {
    setConfirming(null);
    try {
      await deleteClusterSecret(name);
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  }

  return (
    <>
      <PageHeader
        title="Secrets"
        summary="Values a config refers to as { secret: name }, such as the admin token or the gossip key."
        actions={
          <Button variant="primary" disabled={!canWrite} onClick={() => setAdding(true)}>
            Add secret
          </Button>
        }
      />
      {error && <ErrorState onRetry={load}>{error}</ErrorState>}
      {!items && !error && <Loading />}
      {items && (
        <Table>
          <THead>
            <TR>
              <TH>name</TH>
              <TH>key</TH>
              <TH>set</TH>
              <TH>by</TH>
              <TH />
            </TR>
          </THead>
          <TBody>
            {items.map((s) => (
              <TR key={s.name}>
                <TD className="font-mono text-ink">{s.name}</TD>
                <TD className="font-mono text-xs text-ink-muted">{s.key_id}</TD>
                <TD className="text-sm text-ink-muted">{when(s.set_at)}</TD>
                <TD className="text-sm text-ink-muted">{s.actor ?? ""}</TD>
                <TD className="text-right whitespace-nowrap">
                  {confirming === s.name ? (
                    <>
                      <span className="mr-2 text-sm text-ink-muted">Anything using it fails at its next restart.</span>
                      <Button variant="danger" className="mr-2" onClick={() => remove(s.name)}>
                        Yes, delete
                      </Button>
                      <Button variant="ghost" onClick={() => setConfirming(null)}>
                        Keep
                      </Button>
                    </>
                  ) : (
                    <Button variant="danger" disabled={!canWrite} aria-label={`Delete ${s.name}`} onClick={() => setConfirming(s.name)}>
                      Delete
                    </Button>
                  )}
                </TD>
              </TR>
            ))}
          </TBody>
        </Table>
      )}
      <p className="mt-2 text-sm text-ink-muted">
        Values are write-only: once saved they cannot be read back, here or anywhere. A change applies to a proxy at its next restart.
      </p>
      {adding && (
        <AddDialog
          onClose={(saved) => {
            setAdding(false);
            if (saved) load();
          }}
        />
      )}
    </>
  );
}

function AddDialog({ onClose }: { onClose: (saved: boolean) => void }) {
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameOk = validSecretName(name);
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      await putClusterSecret(name, value);
      setValue("");
      onClose(true);
    } catch (err) {
      setError(errorText(err));
      setBusy(false);
    }
  }
  return (
    <Dialog open onOpenChange={(o) => !o && onClose(false)} title="Add secret" description="Saving a name that exists replaces its value.">
      <form className="flex flex-col gap-3" onSubmit={submit}>
        <label className="flex flex-col gap-1 text-sm text-ink-muted">
          Name
          <Input mono value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        {name !== "" && !nameOk && <p className="text-sm text-bad">Use 1 to 63 of a-z, 0-9 and "-", not starting or ending with "-".</p>}
        <label className="flex flex-col gap-1 text-sm text-ink-muted">
          Value
          <Input type="password" autoComplete="off" mono value={value} onChange={(e) => setValue(e.target.value)} />
        </label>
        <p className="text-sm text-ink-muted">Between 8 and 4096 bytes. It is sent to the controller and never shown again.</p>
        {error && <ErrorState>{error}</ErrorState>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onClose(false)}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" loading={busy} disabled={!nameOk || value === ""}>
            Save secret
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
