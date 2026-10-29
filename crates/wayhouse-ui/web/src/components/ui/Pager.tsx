import { Button } from "./Button";
import { Select } from "./Input";

export const PAGE_SIZES = [25, 100];

/** Server-side paging: "Showing 1 to 25 of 500", previous and next, and a page-size select. `page` is 1-based. */
export function Pager({
  page,
  size,
  total,
  onPage,
  onSize,
}: {
  page: number;
  size: number;
  total: number;
  onPage: (page: number) => void;
  onSize: (size: number) => void;
}) {
  const first = total === 0 ? 0 : (page - 1) * size + 1;
  const last = Math.min(page * size, total);
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 py-2 text-sm">
      <span role="status" className="tabular-nums text-ink-muted">
        Showing {first} to {last} of {total}
      </span>
      <span className="flex items-center gap-2">
        <label className="flex items-center gap-1.5 text-ink-muted">
          Per page
          <Select value={size} onChange={(e) => onSize(Number(e.target.value))}>
            {PAGE_SIZES.map((s) => (
              <option key={s}>{s}</option>
            ))}
          </Select>
        </label>
        <Button disabled={page <= 1} onClick={() => onPage(page - 1)}>
          Previous
        </Button>
        <Button disabled={last >= total} onClick={() => onPage(page + 1)}>
          Next
        </Button>
      </span>
    </div>
  );
}
