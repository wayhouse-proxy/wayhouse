import { useEffect } from "react";
import { AlertTriangle } from "lucide-react";
import { Button } from "../ui/Button";

/** Title, one-line summary and page-level actions; also names the browser tab. */
export function PageHeader({
  title,
  summary,
  actions,
}: {
  title: string;
  summary?: React.ReactNode;
  actions?: React.ReactNode;
}) {
  useEffect(() => {
    document.title = `${title} · wayhouse`;
  }, [title]);
  return (
    <header className="mb-4 flex flex-wrap items-start justify-between gap-3">
      <div>
        <h1 className="text-xl font-semibold">{title}</h1>
        {summary && <p className="mt-0.5 max-w-(--measure) text-sm text-ink-muted">{summary}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </header>
  );
}

/** The one most important condition, a line under the page header (components.md, Alert bar). */
export function AlertBar({ children, action }: { children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div role="status" className="mb-4 flex items-center gap-2 border border-line bg-surface px-3 py-2 text-sm">
      <AlertTriangle aria-hidden size={16} strokeWidth={1.5} className="shrink-0 text-warn" />
      <span className="flex-1">{children}</span>
      {action}
    </div>
  );
}

/** Sticky bottom bar of editable pages: "N unsaved changes" with Discard and Apply. */
export function SaveBar({
  changes,
  onDiscard,
  onApply,
  applying,
}: {
  changes: number;
  onDiscard: () => void;
  onApply: () => void;
  applying?: boolean;
}) {
  return (
    <div className="sticky bottom-0 mt-4 flex items-center justify-between gap-3 border-t border-line bg-surface px-3 py-2">
      <span role="status" className="text-sm text-ink-muted">
        {changes === 0 ? "No changes" : `${changes} unsaved ${changes === 1 ? "change" : "changes"}`}
      </span>
      <span className="flex gap-2">
        <Button disabled={changes === 0} onClick={onDiscard}>
          Discard
        </Button>
        <Button variant="primary" disabled={changes === 0} loading={applying} onClick={onApply}>
          Apply
        </Button>
      </span>
    </div>
  );
}
