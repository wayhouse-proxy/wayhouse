import { useEffect, useState } from "react";
import { useFleetSocket } from "../useFleetSocket";
import { ApiError, deleteSniffer, listInstanceSniffers, rollbackSniffer, uploadSniffer } from "../api";
import type { SnifferInfo } from "../types";
import { Select } from "../components/ui/Input";
import { useCan } from "../lib/session";
import { PageHeader } from "../components/shell/Page";
import { EmptyState, ErrorState } from "../components/shell/States";
import { Badge } from "../components/ui/Badge";
import { Button, Input } from "../components/ui/Button";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { Dialog } from "../components/ui/Dialog";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { RegistrySection } from "./RegistrySection";
import { UpdatesSection } from "./UpdatesSection";

export function SniffersPage() {
  const canWrite = useCan("operator");
  const { instances } = useFleetSocket();
  const [selected, setSelected] = useState<string | null>(null);
  const [modules, setModules] = useState<SnifferInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [uploadOpen, setUploadOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const { confirm, dialog } = useConfirm();

  const instanceName = selected ?? instances[0]?.instance ?? null;

  useEffect(() => {
    if (!instanceName) return;
    setError(null);
    listInstanceSniffers(instanceName)
      .then(setModules)
      .catch((err) => {
        setModules(null);
        setError(
          err instanceof ApiError && err.status === 409
            ? "this instance has no settings.sniffers configured"
            : err instanceof ApiError
              ? err.message
              : String(err),
        );
      });
  }, [instanceName, notice]);

  async function handleDelete(name: string) {
    const ok = await confirm({
      title: `Remove ${name}?`,
      description:
        "Deletes the module from every instance's sniffers directory; routes that use it stop matching after the next rescan.",
      confirmLabel: "Remove",
    });
    if (!ok) return;
    setBusy(true);
    try {
      await deleteSniffer(name);
      setNotice(`removed ${name}`);
    } catch (err) {
      setNotice(`remove ${name} failed: ${err instanceof ApiError ? err.message : err}`);
    } finally {
      setBusy(false);
    }
  }

  async function handleRollback(name: string) {
    const ok = await confirm({
      title: `Roll back ${name}?`,
      description:
        "Every instance swaps this module with the version it kept before the last upload. Instances without a kept version are skipped. Pinned instances only roll back to a version their pin allows.",
      confirmLabel: "Roll back",
    });
    if (!ok) return;
    setBusy(true);
    try {
      const res = await rollbackSniffer(name);
      const done = res.results.filter((r) => r.status !== null && r.status >= 200 && r.status < 300).length;
      setNotice(
        done === 0
          ? `roll back ${name} failed on all ${res.results.length} instances`
          : `rolled back ${name} on ${done} of ${res.results.length} instances`,
      );
    } catch (err) {
      setNotice(`roll back ${name} failed: ${err instanceof ApiError ? err.message : err}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="max-w-3xl">
      <PageHeader
        title="Sniffers"
        summary="WASM modules that recognise a game protocol or hostname on a connection."
        actions={
          <Button
            variant="primary"
            disabled={!canWrite}
            title={canWrite ? undefined : "Uploading modules needs the operator role"}
            onClick={() => setUploadOpen(true)}
          >
            Upload module
          </Button>
        }
      />

      <div className="mb-4 flex items-center gap-2 text-sm">
        <label htmlFor="sniffer-instance" className="text-ink-muted">Instance</label>
        <Select id="sniffer-instance" value={instanceName ?? ""} onChange={(e) => setSelected(e.target.value)}>
          {instances.length === 0 && <option>no instances known yet</option>}
          {instances.map((i) => (
            <option key={i.instance} value={i.instance}>
              {i.instance}
            </option>
          ))}
        </Select>
      </div>

      {notice && (
        <p role="status" className="mb-4 text-sm text-ink-muted">{notice}</p>
      )}

      <div>
        {error && <ErrorState>{error}</ErrorState>}
        {!error && modules?.length === 0 && (
          <EmptyState>No modules in this instance's sniffers.dir. Upload one to add it.</EmptyState>
        )}
        {!error && modules && modules.length > 0 && (
          <Table>
            <THead>
              <TR>
                <TH>name</TH>
                <TH>sha256</TH>
                <TH>size</TH>
                <TH>status</TH>
                <TH></TH>
              </TR>
            </THead>
            <TBody>
              {modules.map((m) => (
                <TR key={m.name}>
                  <TD className="font-mono text-ink">{m.name}</TD>
                  <TD className="font-mono text-xs text-ink-muted">{m.sha256.slice(0, 12)}…</TD>
                  <TD className="text-ink-muted">{m.size_bytes} B</TD>
                  <TD>
                    {m.loaded ? <Badge tone="good">loaded</Badge> : <Badge tone="warn">not loaded</Badge>}
                    {m.fallback && (
                      <span
                        className="ml-2"
                        title="The current file failed validation, so the previous version is running"
                      >
                        <Badge tone="warn">fallback active</Badge>
                      </span>
                    )}
                  </TD>
                  <TD className="text-right">
                    {m.has_previous && (
                      <Button size="sm" onClick={() => handleRollback(m.name)} disabled={busy || !canWrite}>
                        roll back
                      </Button>
                    )}
                    <Button variant="danger" size="sm" onClick={() => handleDelete(m.name)} disabled={busy || !canWrite}>
                      remove
                    </Button>
                  </TD>
                </TR>
              ))}
            </TBody>
          </Table>
        )}
      </div>

      <UpdatesSection onChanged={setNotice} />

      <RegistrySection onInstalled={(name) => setNotice(`installed ${name} from a registry`)} />

      {dialog}
      <UploadDialog
        open={uploadOpen}
        onOpenChange={setUploadOpen}
        onUploaded={(name) => setNotice(`uploaded ${name}`)}
      />
    </div>
  );
}

function UploadDialog({
  open,
  onOpenChange,
  onUploaded,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onUploaded: (name: string) => void;
}) {
  const [name, setName] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!file) {
      setError("choose a .wasm file");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const bytes = await file.arrayBuffer();
      const moduleName = name || file.name.replace(/\.wasm$/, "");
      await uploadSniffer(moduleName, bytes);
      onUploaded(moduleName);
      onOpenChange(false);
      setName("");
      setFile(null);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange} title="Upload a sniffer module">
      <form className="flex flex-col gap-3" onSubmit={submit}>
        <label className="flex flex-col gap-1 text-sm text-ink-muted">
          Module name (defaults to the file name)
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. a2s" />
        </label>
        <label className="flex flex-col gap-1 text-sm text-ink-muted">
          .wasm file
          <input
            type="file"
            accept=".wasm"
            onChange={(e) => setFile(e.target.files?.[0] ?? null)}
            className="text-sm text-ink"
          />
        </label>
        {error && <ErrorState>{error}</ErrorState>}
        <div className="mt-2 flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={busy}>
            {busy ? "Uploading…" : "Upload to every instance"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
