import { useEffect, useState } from "react";
import { cn } from "../../lib/cn";
import { Button } from "../ui/Button";
import { StatusMark } from "../ui/StatusMark";
import { AlertBar } from "./Page";

/** First use: what will appear here and the one action that fills it. Left-aligned, no illustration. */
export function EmptyState({ children, action }: { children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="py-4 text-sm text-ink-muted">
      <p className="max-w-(--measure)">{children}</p>
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}

export const Skeleton = ({ className }: { className?: string }) => (
  <div aria-hidden className={cn("h-8 animate-pulse bg-surface-sunken motion-reduce:animate-none", className)} />
);

/** Skeleton rows at the final size; "Still loading" appears after 8 seconds. */
export function Loading({ rows = 3 }: { rows?: number }) {
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setSlow(true), 8000);
    return () => clearTimeout(t);
  }, []);
  return (
    <div role="status" aria-label="Loading" className="flex flex-col gap-px">
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} />
      ))}
      {slow && <p className="pt-2 text-sm text-ink-muted">Still loading</p>}
    </div>
  );
}

/** What failed, the likely cause, the next step; Retry when there is one. */
export function ErrorState({ children, onRetry }: { children: React.ReactNode; onRetry?: () => void }) {
  return (
    <div role="alert" className="py-4 text-sm">
      <StatusMark state="down" word="Failed" />
      <p className="mt-1 max-w-(--measure) text-ink-muted">{children}</p>
      {onRetry && (
        <Button size="sm" className="mt-2" onClick={onRetry}>
          Retry
        </Button>
      )}
    </div>
  );
}

/** A persistent bar while live updates are down. */
export function DisconnectedBar({ since, onReconnect }: { since?: string; onReconnect?: () => void }) {
  return (
    <AlertBar action={onReconnect && <Button size="sm" onClick={onReconnect}>Reconnect</Button>}>
      Live updates stopped.{since ? ` Showing data from ${since}.` : ""}
    </AlertBar>
  );
}

/** For a page the session's role may not use. */
export function Forbidden() {
  return (
    <div role="alert" className="py-4 text-sm">
      <StatusMark state="down" word="Not allowed" />
      <p className="mt-1 text-ink-muted">Your role cannot use this page. Ask an administrator for access.</p>
    </div>
  );
}
