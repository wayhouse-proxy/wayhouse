import { useCallback, useEffect, useState } from "react";
import { ApiError, deleteClusterSecret, getPluginSecretKeys, listClusterSecrets, putClusterSecret, rewrapPluginSecrets, type PluginSecretKeys } from "../api";
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
      {canWrite && <KeyRotation />}
      {adding && (
        <AddDialog
          taken={new Set((items ?? []).map((s) => s.name))}
          onClose={(saved) => {
            setAdding(false);
            if (saved) load();
          }}
        />
      )}
    </>
  );
}

function AddDialog({ taken, onClose }: { taken: Set<string>; onClose: (saved: boolean) => void }) {
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameOk = validSecretName(name);
  const exists = taken.has(name);
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
        {exists && (
          <p className="text-sm text-bad">
            {name} exists. Saving replaces its value; the old one cannot be recovered, and proxies using it pick up the new one at their next restart.
          </p>
        )}
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
            {exists ? "Replace secret" : "Save secret"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

/** Key rotation for the plugin secrets: which key ids each controller holds, and the re-encrypt step once all hold the active one. */
function KeyRotation() {
  const [keys, setKeys] = useState<PluginSecretKeys | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // A controller without plugins has no plugin secrets to rotate: show nothing, not an error.
  const [absent, setAbsent] = useState(false);

  const load = useCallback(async () => {
    try {
      setKeys(await getPluginSecretKeys());
      setProblem(null);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) setAbsent(true);
      else setProblem(errorText(err));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  async function rewrap() {
    setBusy(true);
    setDone(null);
    try {
      const r = await rewrapPluginSecrets();
      const left = r.skipped.length + r.unreadable.length;
      setDone(`Re-encrypted ${r.rewrapped} plugin secret${r.rewrapped === 1 ? "" : "s"} under ${r.active_key_id ?? "the active key"}.${left ? ` ${left} could not be re-encrypted: ${[...r.skipped, ...r.unreadable].join(", ")}.` : ""}`);
      await load();
    } catch (err) {
      setProblem(errorText(err));
    } finally {
      setBusy(false);
    }
  }

  if (absent) return null;
  const rows = keys ? (Object.keys(keys.nodes).length ? Object.entries(keys.nodes) : [["this controller", keys.key_ids] as const]) : [];
  const lacking = keys?.active ? rows.filter(([, ids]) => Array.isArray(ids) && !ids.includes(keys.active!)).map(([n]) => n) : [];
  return (
    <section aria-label="Key rotation" className="mt-8">
      <h2 className="mb-1 text-lg font-semibold">Key rotation</h2>
      <p className="mb-2 max-w-(--measure) text-sm text-ink-muted">
        Plugin secrets are sealed under the controller key. To rotate: add the new key to every controller&apos;s key file, restart them, check below that every node holds the active key, then re-encrypt. Remove the old key only after that.
      </p>
      {problem && <ErrorState onRetry={load}>{problem}</ErrorState>}
      {keys && (
        <Table aria-label="Keys per node">
          <THead>
            <TR>
              <TH>node</TH>
              <TH>key ids</TH>
            </TR>
          </THead>
          <TBody>
            {rows.map(([node, ids]) => (
              <TR key={node}>
                <TD className="text-ink">{node}</TD>
                <TD className="font-mono text-xs">{Array.isArray(ids) ? ids.map((i) => (i === keys.active ? `${i} (active)` : i)).join(", ") : <span className="text-bad">{ids.error}</span>}</TD>
              </TR>
            ))}
          </TBody>
        </Table>
      )}
      {lacking.map((n) => (
        <p key={n} role="status" className="mt-1 text-sm font-semibold">
          Node {n} does not hold the active key {keys!.active}. Re-encrypting now would leave it unable to read its plugin secrets.
        </p>
      ))}
      {done && (
        <p role="status" className="mt-2 text-sm">
          {done}
        </p>
      )}
      <Button className="mt-2" loading={busy} disabled={!keys || lacking.length > 0} onClick={rewrap}>
        Re-encrypt plugin secrets
      </Button>
    </section>
  );
}
