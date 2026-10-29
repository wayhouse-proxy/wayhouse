import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle } from "lucide-react";
import { confirmRevision, revertNow } from "../../api";
import { PENDING_KEY, usePending } from "../../lib/config/usePending";
import { useCan } from "../../lib/session";
import { Button } from "../ui/Button";

/** What a screen reader hears: the first message, then only when 30, 10 and 0 seconds are crossed. */
function spoken(level: 0 | 10 | 30 | null, revertTo: number): string {
  if (level === 0) return `Time is up. The controller is reverting to revision ${revertTo}.`;
  if (level === 10) return "10 seconds left to confirm.";
  if (level === 30) return "30 seconds left to confirm.";
  return `Applied on trial. Confirm before the countdown ends, or the controller reverts to revision ${revertTo}.`;
}

/**
 * A revision on trial (config confirm and revert, docs/06). The deadline is the controller's, so this page,
 * another operator's page and a closed tab all agree; the buttons only end the trial early.
 */
export function TrialBar() {
  const canWrite = useCan("admin");
  const qc = useQueryClient();
  const { data, dataUpdatedAt } = usePending();
  const [, tick] = useState(0);
  const pending = data?.pending ?? null;

  useEffect(() => {
    if (!pending) return;
    const t = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, [pending?.revision]); // eslint-disable-line react-hooks/exhaustive-deps

  const end = useMutation({
    mutationFn: (keep: boolean) => (keep ? confirmRevision(pending!.revision) : revertNow(pending!.revision)),
    onSettled: () => qc.invalidateQueries({ queryKey: PENDING_KEY }),
  });

  const remainingMs = pending ? Math.max(0, pending.remaining_ms - (Date.now() - dataUpdatedAt)) : 0;
  // The announcement changes only when a threshold is crossed, so the live region stays quiet in between.
  const level = remainingMs <= 0 ? 0 : remainingMs <= 10_000 ? 10 : remainingMs <= 30_000 ? 30 : null;
  useEffect(() => {
    if (pending && remainingMs <= 0) qc.invalidateQueries({ queryKey: PENDING_KEY });
  }, [pending?.revision, remainingMs <= 0]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!pending) return null;

  const secs = Math.ceil(remainingMs / 1000);
  const why = data!.risks.filter((r) => pending.reasons.includes(r.id)).map((r) => r.text);
  return (
    <div className="mb-4 flex flex-wrap items-center gap-2 border border-warn bg-surface px-3 py-2 text-sm">
      <AlertTriangle aria-hidden size={16} strokeWidth={1.5} className="shrink-0 text-warn" />
      <span className="sr-only" role="status">
        {spoken(level, pending.revert_to)}
      </span>
      <span aria-hidden className="flex-1">
        Applied as revision {pending.revision}. Confirm within <strong className="font-mono">{secs} s</strong>, or the controller reverts
        to revision {pending.revert_to}.{why.length > 0 && ` (${why.join("; ")}.)`}
      </span>
      <Button variant="primary" disabled={!canWrite} loading={end.isPending} onClick={() => end.mutate(true)}>
        Keep changes
      </Button>
      <Button disabled={!canWrite} onClick={() => end.mutate(false)}>
        Revert now
      </Button>
      {end.isError && <span role="alert" className="w-full text-bad">{end.error instanceof Error ? end.error.message : String(end.error)}</span>}
    </div>
  );
}
