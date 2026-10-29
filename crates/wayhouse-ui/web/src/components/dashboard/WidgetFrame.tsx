import type { UseQueryResult } from "@tanstack/react-query";
import { Component } from "react";
import { ErrorState, Skeleton } from "../shell/States";

/** A panel with the widget contract (components.md): title, body, and a footer that says how fresh the data is. */
export function Panel({ title, footer, children }: { title: string; footer?: React.ReactNode; children: React.ReactNode }) {
  return (
    <section aria-label={title} className="flex h-full flex-col rounded-sm border border-line bg-surface">
      <header className="flex h-[38px] items-center border-b border-line px-3">
        <h2 className="text-sm font-semibold">{title}</h2>
      </header>
      <div className="flex-1 p-3">{children}</div>
      {footer && <footer className="border-t border-line px-3 py-1.5 text-xs text-ink-muted">{footer}</footer>}
    </section>
  );
}

export const updated = (ms: number) => {
  if (!ms) return undefined;
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return s < 90 ? `Updated ${s} s ago` : `Updated ${Math.round(s / 60)} min ago`;
};

/** Loading is a skeleton at the final size, an error names what failed and retries without touching the other widgets. */
export function Async<T>({ q, children, empty }: { q: UseQueryResult<T>; children: (data: T) => React.ReactNode; empty?: (data: T) => string | null }) {
  if (q.isPending) return <Skeleton className="h-24 w-full" />;
  if (q.error) return <ErrorState onRetry={() => q.refetch()}>{(q.error as Error).message}</ErrorState>;
  const none = empty?.(q.data as T);
  if (none) return <p className="py-4 text-sm text-ink-muted">{none}</p>;
  return <>{children(q.data as T)}</>;
}

/** 26 px semibold tabular, with its unit or context in muted text; it always sits above or beside its history. */
export const Headline = ({ value, unit }: { value: React.ReactNode; unit?: string }) => (
  <p className="mb-2 flex items-baseline gap-1.5">
    <span className="text-[26px] leading-none font-semibold tabular-nums">{value}</span>
    {unit && <span className="text-[13px] text-ink-muted">{unit}</span>}
  </p>
);

/** The "Showing 5 of 49" line with a link to the full view. */
export function Showing({ shown, total, children }: { shown: number; total: number; children?: React.ReactNode }) {
  return (
    <p className="mt-2 text-xs text-ink-muted">
      Showing {shown} of {total}
      {children && <> · {children}</>}
    </p>
  );
}

/** A widget that throws shows its own error; the dashboard and the other widgets keep working. */
export class WidgetBoundary extends Component<{ title: string; children: React.ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  render() {
    if (!this.state.error) return this.props.children;
    return (
      <Panel title={this.props.title}>
        <ErrorState onRetry={() => this.setState({ error: null })}>This widget failed to draw: {this.state.error.message}</ErrorState>
      </Panel>
    );
  }
}
