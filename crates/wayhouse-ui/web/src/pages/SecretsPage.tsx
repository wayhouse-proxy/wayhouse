import { useCallback, useEffect, useState } from "react";
import { ApiError, deleteClusterSecret, getPluginSecretKeys, listClusterSecrets, putClusterSecret, rewrapAcmeKeys, rewrapClusterSecrets, rewrapPluginSecrets, type PluginSecretKeys, type RewrapResult } from "../api";
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
  const size = new TextEncoder().encode(value).length;
  const sizeOk = size >= 8 && size <= 4096;
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
        <p className={value !== "" && !sizeOk ? "text-sm text-bad" : "text-sm text-ink-muted"}>
          Between 8 and 4096 bytes{value !== "" ? ` (now ${size})` : ""}. It is sent to the controller and never shown again.
        </p>
        {error && <ErrorState>{error}</ErrorState>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onClose(false)}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" loading={busy} disabled={!nameOk || !sizeOk}>
            {exists ? "Replace secret" : "Save secret"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

function omit(d: Record<string, string>, key: string): Record<string, string> {
  return Object.fromEntries(Object.entries(d).filter(([k]) => k !== key));
}

/** Key rotation: which key ids each controller holds, and the re-encrypt steps (plugin secrets, cluster secrets, certificate keys) once all hold the active one. */
function KeyRotation() {
  const [keys, setKeys] = useState<PluginSecretKeys | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  // The last result per kind, so all three stay visible while an operator works through them.
  const [done, setDone] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
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

  async function rewrap(what: string, run: () => Promise<RewrapResult>) {
    setBusy(what);
    setDone((d) => omit(d, what));
    try {
      const r = await run();
      const left = r.skipped.length + r.unreadable.length;
      const text = `Re-encrypted ${r.rewrapped} ${what}${r.rewrapped === 1 ? "" : "s"} under ${r.active_key_id ?? "the active key"}.`;
      setDone((d) => ({ ...d, [what]: left ? `${text} ${left} could not be re-encrypted (${[...r.skipped, ...r.unreadable].join(", ")}): keep the old key until they are.` : text }));
      await load();
    } catch (err) {
      // Only an ACME-less controller answers 501: it has no certificate keys. Any other failure,
      // a 404 included, is an error: "nothing to re-encrypt" could get the old key removed too early.
      if (what === "certificate key" && err instanceof ApiError && err.status === 501) setDone((d) => ({ ...d, [what]: "This controller has no certificate keys to re-encrypt." }));
      else setProblem(`${what}: ${errorText(err)}`);
    } finally {
      setBusy(null);
    }
  }

  if (absent) return null;
  const rows = keys ? (Object.keys(keys.nodes ?? {}).length ? Object.entries(keys.nodes) : [["this controller", keys.key_ids] as const]) : [];
  const lacking = keys?.active ? rows.filter(([, ids]) => Array.isArray(ids) && !ids.includes(keys.active!)).map(([n]) => n) : [];
  return (
    <section aria-label="Key rotation" className="mt-8">
      <h2 className="mb-1 text-lg font-semibold">Key rotation</h2>
      <p className="mb-2 max-w-(--measure) text-sm text-ink-muted">
        Plugin secrets, cluster secrets and ACME keys are sealed under the controller key. To rotate: add the new key to every controller&apos;s key file, restart them, check below that every node holds the active key, then re-encrypt each kind below. Remove the old key only after all three. Nodes that are unreachable are skipped: check the table above before you remove it.
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
      {Object.entries(done).map(([what, text]) => (
        <p key={what} role="status" className="mt-2 text-sm">
          {text}
        </p>
      ))}
      <div className="mt-2 flex flex-wrap gap-2">
        <Button loading={busy === "plugin secret"} disabled={!keys || lacking.length > 0 || busy !== null} onClick={() => rewrap("plugin secret", rewrapPluginSecrets)}>
          Re-encrypt plugin secrets
        </Button>
        <Button loading={busy === "cluster secret"} disabled={!keys || lacking.length > 0 || busy !== null} onClick={() => rewrap("cluster secret", rewrapClusterSecrets)}>
          Re-encrypt cluster secrets
        </Button>
        <Button loading={busy === "certificate key"} disabled={!keys || lacking.length > 0 || busy !== null} onClick={() => rewrap("certificate key", rewrapAcmeKeys)}>
          Re-encrypt certificate keys
        </Button>
      </div>
    </section>
  );
}
