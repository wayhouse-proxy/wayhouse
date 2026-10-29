import { useMemo, useState } from "react";
import {
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type SortingState,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, ChevronRight } from "lucide-react";
import { cn } from "../../lib/cn";
import { Table, TBody, TD, TH, THead, TR } from "./Table";

/** Rows above this count are virtualised (design-system scale rule). */
export const VIRTUALISE_ABOVE = 200;
const ROW_PX = 32;

export interface Group<T> {
  key: string;
  label: string;
  rollup?: React.ReactNode;
  rows: T[];
}

/**
 * TanStack Table over the design-system table: sortable headers (buttons with `aria-sort`,
 * one sort at a time), optional collapsible groups with a rollup row, a selected row that opens
 * the inspector on click or Enter, and virtualisation above 200 plain rows.
 * Paging and filtering belong to the server; see Pager.
 */
export function DataTable<T>({
  columns,
  data,
  groups,
  getRowId,
  selectedId,
  onOpen,
}: {
  columns: ColumnDef<T>[];
  data: T[];
  /** Grouped display (region, then node); `data` is ignored when given. */
  groups?: Group<T>[];
  getRowId: (row: T) => string;
  selectedId?: string | null;
  onOpen?: (row: T) => void;
}) {
  const [sorting, setSorting] = useState<SortingState>([]);
  const flat = useMemo(() => groups?.flatMap((g) => g.rows) ?? data, [groups, data]);
  const table = useReactTable({
    data: flat,
    columns,
    getRowId,
    state: { sorting },
    onSortingChange: (u) => setSorting((s) => (typeof u === "function" ? u(s) : u).slice(-1)),
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    enableMultiSort: false,
  });
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const sorted = table.getRowModel().rows;
  const virtual = !groups && sorted.length > VIRTUALISE_ABOVE;
  const [scroller, setScroller] = useState<HTMLDivElement | null>(null);
  const virt = useVirtualizer({ count: sorted.length, getScrollElement: () => scroller, estimateSize: () => ROW_PX, overscan: 10, enabled: virtual });

  const cells = (row: (typeof sorted)[number]) =>
    row.getVisibleCells().map((c) => (
      <TD key={c.id} numeric={(c.column.columnDef.meta as { numeric?: boolean } | undefined)?.numeric}>
        {flexRender(c.column.columnDef.cell, c.getContext())}
      </TD>
    ));
  const rowProps = (row: (typeof sorted)[number]) => ({
    selected: row.id === selectedId,
    tabIndex: onOpen ? 0 : undefined,
    onClick: onOpen ? () => onOpen(row.original) : undefined,
    onKeyDown: onOpen
      ? (e: React.KeyboardEvent) => {
          if (e.key === "Enter" && e.target === e.currentTarget) onOpen(row.original);
        }
      : undefined,
    className: onOpen ? "cursor-pointer" : undefined,
  });

  const header = (
    <THead>
      {table.getHeaderGroups().map((hg) => (
        <TR key={hg.id} className="border-0 hover:bg-transparent">
          {hg.headers.map((h) => {
            const dir = h.column.getIsSorted();
            const numeric = (h.column.columnDef.meta as { numeric?: boolean } | undefined)?.numeric;
            return (
              <TH key={h.id} numeric={numeric} sort={h.column.getCanSort() ? (dir === "asc" ? "ascending" : dir === "desc" ? "descending" : "none") : undefined}>
                {h.column.getCanSort() ? (
                  <button type="button" onClick={h.column.getToggleSortingHandler()} className="inline-flex items-center gap-1 font-semibold">
                    {flexRender(h.column.columnDef.header, h.getContext())}
                    {dir === "asc" && <ArrowUp aria-hidden size={12} strokeWidth={1.5} />}
                    {dir === "desc" && <ArrowDown aria-hidden size={12} strokeWidth={1.5} />}
                  </button>
                ) : (
                  flexRender(h.column.columnDef.header, h.getContext())
                )}
              </TH>
            );
          })}
        </TR>
      ))}
    </THead>
  );

  if (virtual) {
    const items = virt.getVirtualItems();
    const pad = items.length ? [items[0].start, virt.getTotalSize() - items[items.length - 1].end] : [0, 0];
    return (
      <div ref={setScroller} className="max-h-[70vh] overflow-auto">
        <Table>
          {header}
          <TBody>
            {pad[0] > 0 && <tr style={{ height: pad[0] }} aria-hidden />}
            {items.map((v) => {
              const row = sorted[v.index];
              return (
                <TR key={row.id} {...rowProps(row)}>
                  {cells(row)}
                </TR>
              );
            })}
            {pad[1] > 0 && <tr style={{ height: pad[1] }} aria-hidden />}
          </TBody>
        </Table>
      </div>
    );
  }

  const byId = new Map(sorted.map((r) => [r.id, r]));
  return (
    <Table>
      {header}
      <TBody>
        {groups
          ? groups.map((g) => {
              const open = !collapsed.has(g.key);
              return [
                <tr key={g.key} className="h-(--row-height) border-t border-line bg-surface-sunken font-semibold">
                  <td colSpan={columns.length} className="px-3">
                    <button
                      type="button"
                      aria-expanded={open}
                      onClick={() => setCollapsed((s) => { const n = new Set(s); if (open) n.add(g.key); else n.delete(g.key); return n; })}
                      className="flex w-full items-center gap-1.5 text-left"
                    >
                      <ChevronRight aria-hidden size={16} strokeWidth={1.5} className={cn("transition-transform duration-(--dur-fast)", open && "rotate-90")} />
                      {g.label}
                      {g.rollup && <span className="ml-2 text-xs font-normal text-ink-muted tabular-nums">{g.rollup}</span>}
                    </button>
                  </td>
                </tr>,
                ...(open
                  ? g.rows.map((r) => byId.get(getRowId(r))).filter((r): r is NonNullable<typeof r> => !!r).map((row) => (
                      <TR key={row.id} {...rowProps(row)}>{cells(row)}</TR>
                    ))
                  : []),
              ];
            })
          : sorted.map((row) => (
              <TR key={row.id} {...rowProps(row)}>
                {cells(row)}
              </TR>
            ))}
      </TBody>
    </Table>
  );
}
