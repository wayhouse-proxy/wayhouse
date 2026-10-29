import { cn } from "../../lib/cn";

/** Styled table parts: 32 px rows, muted semibold header on the sunken ground, hairlines, no zebra. Scrolls inside its own container. */
export function Table({ className, ...props }: React.TableHTMLAttributes<HTMLTableElement>) {
  return (
    <div className="max-w-full overflow-x-auto rounded-sm border border-line bg-surface">
      <table className={cn("w-full border-collapse text-left text-sm", className)} {...props} />
    </div>
  );
}

export const THead = (p: React.HTMLAttributes<HTMLTableSectionElement>) => <thead {...p} />;
export const TBody = (p: React.HTMLAttributes<HTMLTableSectionElement>) => <tbody {...p} />;

export function TR({ selected, className, ...props }: React.HTMLAttributes<HTMLTableRowElement> & { selected?: boolean }) {
  return (
    <tr
      aria-selected={selected || undefined}
      className={cn(
        "h-(--row-height) border-t border-line first:border-t-0 hover:bg-ink/[0.04]",
        selected && "bg-select-bg",
        className,
      )}
      {...props}
    />
  );
}

export function TH({
  sort,
  numeric,
  className,
  children,
  ...props
}: React.ThHTMLAttributes<HTMLTableCellElement> & { sort?: "ascending" | "descending" | "none"; numeric?: boolean }) {
  return (
    <th
      scope="col"
      aria-sort={sort}
      className={cn(
        "h-8 bg-surface-sunken px-3 text-xs font-semibold text-ink-muted",
        numeric && "text-right",
        className,
      )}
      {...props}
    >
      {children}
    </th>
  );
}

export function TD({ numeric, className, ...props }: React.TdHTMLAttributes<HTMLTableCellElement> & { numeric?: boolean }) {
  return <td className={cn("px-3", numeric && "text-right tabular-nums", className)} {...props} />;
}
