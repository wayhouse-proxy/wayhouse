import { useEffect, useState } from "react";
import type { PluginCapabilities } from "../../types";
import { CapabilityList } from "../../lib/plugins/capabilities";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { ErrorState } from "../shell/States";

/**
 * The trust prompt: what the plugin will be allowed to do, and a tick before it is allowed.
 * With `before` it shows what changes (an options edit that moves the approval).
 */
export function ApprovalDialog({
  open,
  title,
  intro,
  caps,
  before,
  confirmLabel,
  busy,
  error,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  intro: string;
  caps: PluginCapabilities;
  before?: PluginCapabilities;
  confirmLabel: string;
  busy?: boolean;
  error?: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const [ticked, setTicked] = useState(false);
  // The tick belongs to the set shown: a new set (or a reopened prompt) asks again.
  useEffect(() => setTicked(false), [open, caps]);
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onCancel()} title={title}>
      <div className="flex flex-col gap-3">
        <p className="text-sm text-ink-muted">{intro}</p>
        {before && (
          <div>
            <p className="mb-1 text-sm font-medium text-ink">Approved now:</p>
            <CapabilityList caps={before} />
          </div>
        )}
        <div>
          <p className="mb-1 text-sm font-medium text-ink">{before ? "Would be approved:" : "This plugin asks to:"}</p>
          <CapabilityList caps={caps} />
        </div>
        <label className="flex items-start gap-2 text-sm text-ink">
          <input type="checkbox" className="mt-1" checked={ticked} onChange={(e) => setTicked(e.target.checked)} />
          I approve these capabilities for this exact module
        </label>
        {error && <ErrorState>{error}</ErrorState>}
        <div className="mt-2 flex justify-end gap-2">
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!ticked} loading={busy} onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
