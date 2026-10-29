import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { structuredPatch } from "diff";
import type { ConfigDoc } from "../../lib/config/useConfigDoc";
import { PENDING_KEY, usePending } from "../../lib/config/usePending";
import { useCan } from "../../lib/session";
import { SaveBar } from "../shell/Page";
import { ErrorState } from "../shell/States";
import { Button, Input } from "../ui/Button";
import { Dialog } from "../ui/Dialog";

/** The YAML change as hunks of two lines of context, the way `diff -u` shows it. */
function Diff({ before, after }: { before: string; after: string }) {
  const hunks = structuredPatch("saved", "edit", before, after, "", "", { context: 2 }).hunks;
  if (hunks.length === 0) return <p className="text-sm text-ink-muted">No difference in the YAML.</p>;
  return (
    <pre aria-label="Changes to the YAML" className="max-h-72 overflow-auto border border-line bg-surface-sunken p-2 font-mono text-xs">
      {hunks.map((h) => (
        <div key={h.oldStart} className="mb-2 last:mb-0">
          <div className="text-ink-muted">{`@@ line ${h.oldStart} @@`}</div>
          {h.lines.map((l, i) => (
            <div key={i} className={l[0] === "+" ? "bg-good/15 text-ink" : l[0] === "-" ? "bg-bad/15 text-ink" : "text-ink-muted"}>
              {l || " "}
            </div>
          ))}
        </div>
      ))}
    </pre>
  );
}

type Check = { state: "checking" } | { state: "ok"; schemaVersion?: number } | { state: "bad"; message: string };

/** Sticky save bar plus the review step: what changes, the controller's check, who gets it, then Apply. */
export function ConfigSaveBar({ doc, onApplied }: { doc: ConfigDoc; onApplied?: (revision: number) => void }) {
  const canWrite = useCan("admin");
  const qc = useQueryClient();
  const pending = usePending();
  const [open, setOpen] = useState(false);
  const [check, setCheck] = useState<Check>({ state: "checking" });
  const [canary, setCanary] = useState(false);
  const [group, setGroup] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<{ message: string; stale: boolean } | null>(null);
  const [done, setDone] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setCheck({ state: "checking" });
    setFailure(null);
    let live = true;
    doc.validate().then((r) => live && setCheck(r.ok ? { state: "ok", schemaVersion: r.schemaVersion } : { state: "bad", message: r.message ?? "rejected" }));
    return () => {
      live = false;
    };
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  async function apply() {
    setBusy(true);
    const r = await doc.apply(canary ? { canaryGroup: group.trim() } : {});
    setBusy(false);
    if (!r.ok) return setFailure({ message: r.message, stale: r.stale });
    setOpen(false);
    setDone(canary ? `Revision ${r.revision} applied to canary group ${group.trim()} only. Promote it from Config history.` : `Applied as revision ${r.revision}.`);
    qc.invalidateQueries({ queryKey: PENDING_KEY });
    onApplied?.(r.revision);
  }

  const risks = pending.data?.risks ?? [];
  const blocked = check.state !== "ok" || (canary && group.trim() === "");
  return (
    <>
      {done && <p role="status" className="mt-2 text-sm text-ink-muted">{done}</p>}
      {canWrite ? (
        <SaveBar changes={doc.changes} onDiscard={() => { doc.discard(); setDone(null); }} onApply={() => { setDone(null); setOpen(true); }} />
      ) : (
        <p className="mt-4 text-sm text-ink-muted">Changing the configuration needs the admin role.</p>
      )}
      <Dialog open={open} onOpenChange={setOpen} title="Apply this configuration?" wide
        description="This submits one new revision. Every instance picks it up, and you can roll back from Config history.">
        <div className="flex flex-col gap-3">
          <Diff before={doc.savedText} after={doc.text} />
          {check.state === "checking" && <p role="status" className="text-sm text-ink-muted">Checking with the controller…</p>}
          {check.state === "ok" && <p role="status" className="text-sm text-ink-muted">The controller accepts this configuration{check.schemaVersion ? ` (schema version ${check.schemaVersion})` : ""}.</p>}
          {check.state === "bad" && <ErrorState>{check.message}</ErrorState>}
          {risks.length > 0 && (
            <p className="text-sm text-ink-muted">
              A change to any of these applies on trial for {pending.data?.auto_confirm_secs} s and reverts unless you confirm:{" "}
              {risks.map((r) => r.text).join("; ")}.
            </p>
          )}
          <label className="flex items-center gap-2 text-sm text-ink">
            <input type="checkbox" checked={canary} onChange={(e) => setCanary(e.target.checked)} />
            Canary: show this revision only to one group first
          </label>
          {canary && (
            <label className="flex flex-col gap-1 text-sm text-ink-muted">
              Group name
              <Input mono value={group} onChange={(e) => setGroup(e.target.value)} placeholder="eu/frankfurt" />
            </label>
          )}
          {failure && (
            <div role="alert" className="text-sm text-bad">
              {failure.stale ? "Someone else changed the configuration after you loaded it. Reload to see their revision; your edit is discarded, so copy it from the Source page first if you want to keep it." : failure.message}
              {failure.stale && (
                <div className="mt-2">
                  <Button onClick={async () => { await doc.reload(); setOpen(false); }}>Reload the latest</Button>
                </div>
              )}
            </div>
          )}
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setOpen(false)}>Cancel</Button>
            <Button variant="primary" loading={busy} disabled={blocked} onClick={apply}>Apply</Button>
          </div>
        </div>
      </Dialog>
    </>
  );
}
