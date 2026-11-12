import type { EnrollmentCode } from "../../types";
import { Badge } from "../ui/Badge";
import { Button } from "../ui/Button";
import { Table, TBody, TD, TH, THead, TR } from "../ui/Table";
import { EmptyState } from "../shell/States";

/** "in 12 min" / "3 h ago" from unix seconds, for the table; the cell's title has the exact time. */
export function relative(unixSecs: number, nowMs = Date.now()): string {
  const diff = unixSecs - Math.floor(nowMs / 1000);
  const abs = Math.abs(diff);
  const span = abs < 60 ? "under a minute" : abs < 3600 ? `${Math.floor(abs / 60)} min` : abs < 86_400 ? `${Math.floor(abs / 3600)} h` : `${Math.floor(abs / 86_400)} d`;
  return diff >= 0 ? `in ${span}` : `${span} ago`;
}

const TONE = { pending: "warn", used: "good", expired: "neutral" } as const;

/** Every code the controller still holds, never the code itself. Revoking works on any row. */
export function PendingCodes({
  items,
  canWrite,
  busy,
  onRevoke,
}: {
  items: EnrollmentCode[];
  canWrite: boolean;
  busy: boolean;
  onRevoke: (c: EnrollmentCode) => void;
}) {
  if (items.length === 0) return <EmptyState>No codes. A code you create appears here until it is revoked.</EmptyState>;
  return (
    <Table>
      <THead>
        <TR>
          <TH>name</TH>
          <TH>role</TH>
          <TH>status</TH>
          <TH>uses left</TH>
          <TH>expires</TH>
          <TH>created by</TH>
          <TH />
        </TR>
      </THead>
      <TBody>
        {items.map((c) => (
          <TR key={c.id}>
            <TD className="font-mono text-ink">{c.origin}</TD>
            <TD className="text-ink-muted">{c.role}</TD>
            <TD>
              <Badge tone={TONE[c.status]}>{c.status}</Badge>
            </TD>
            <TD className="tabular-nums text-ink-muted">
              {c.uses_left} of {c.uses}
            </TD>
            <TD className="text-ink-muted" title={new Date(c.expires_at * 1000).toISOString()}>
              {relative(c.expires_at)}
            </TD>
            <TD className="text-ink-muted">{c.created_by ?? "-"}</TD>
            <TD className="text-right">
              <Button
                variant="danger"
                disabled={busy || !canWrite}
                title={canWrite ? undefined : "Revoking a code needs the admin role"}
                aria-label={`Revoke code for ${c.origin}`}
                onClick={() => onRevoke(c)}
              >
                Revoke
              </Button>
            </TD>
          </TR>
        ))}
      </TBody>
    </Table>
  );
}
