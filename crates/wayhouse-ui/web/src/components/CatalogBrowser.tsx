import { useState } from "react";
import { useSearchParams } from "react-router-dom";
import { applyFilter, facets, filterFromParams, filterToParams, PAGE_SIZE, type CatalogFilter, type CatalogItem, type CatalogSort } from "../lib/catalog";
import { cn } from "../lib/cn";
import { EmptyState } from "./shell/States";
import { Button, Input } from "./ui/Button";
import { Select } from "./ui/Input";

const TOP_TAGS = 12;

function Chip({ on, onClick, children }: { on: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      className={cn(
        "rounded-sm border px-2 py-0.5 text-xs transition-colors",
        on ? "border-primary bg-primary text-primary-ink" : "border-control bg-surface text-ink hover:bg-surface-sunken",
      )}
    >
      {children}
    </button>
  );
}

const toggle = (list: string[], v: string) => (list.includes(v) ? list.filter((x) => x !== v) : [...list, v]);

/**
 * Search, category and tag chips, a signed-only switch and a sort over a registry catalog, with the
 * filter kept in the URL query. Everything runs on the catalog already in the browser. "Popular" is
 * offered only when some entry carries a download count.
 */
export function CatalogBrowser<T extends CatalogItem>({ items, children }: { items: T[]; children: (page: T[]) => React.ReactNode }) {
  const [params, setParams] = useSearchParams();
  const [allTags, setAllTags] = useState(false);
  const canPopular = items.some((i) => i.downloads !== undefined);
  const filter = filterFromParams(params, canPopular);
  const result = applyFilter(items, filter);
  const { categories, tags } = facets(items);
  const shownTags = allTags ? tags : tags.slice(0, TOP_TAGS);

  // A change of filter goes back to page 1; paging itself passes the page explicitly.
  const set = (patch: Partial<CatalogFilter>) =>
    setParams((cur) => filterToParams({ ...filterFromParams(cur, canPopular), page: 1, ...patch }, cur), { replace: true });
  const filtered = filter.q || filter.categories.length || filter.tags.length || filter.signedOnly;

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <label className="flex items-center gap-1.5 text-ink-muted">
          Search
          <Input
            type="search"
            className="w-56"
            value={filter.q}
            placeholder="name, description or tag"
            onChange={(e) => set({ q: e.target.value })}
          />
        </label>
        <label className="flex items-center gap-1.5 text-ink-muted">
          <input type="checkbox" checked={filter.signedOnly} onChange={(e) => set({ signedOnly: e.target.checked })} />
          Signed only
        </label>
        <label className="flex items-center gap-1.5 text-ink-muted">
          Sort
          <Select value={filter.sort} onChange={(e) => set({ sort: e.target.value as CatalogSort })}>
            <option value="name">name</option>
            <option value="updated">recently updated</option>
            {canPopular && <option value="popular">popular</option>}
          </Select>
        </label>
        {filtered && (
          <Button variant="ghost" size="sm" onClick={() => set({ q: "", categories: [], tags: [], signedOnly: false })}>
            Clear filters
          </Button>
        )}
      </div>
      {categories.length > 0 && (
        <div role="group" aria-label="Categories" className="flex flex-wrap items-center gap-1.5">
          <span className="text-xs text-ink-muted">Categories</span>
          {categories.map(([c, n]) => (
            <Chip key={c} on={filter.categories.includes(c)} onClick={() => set({ categories: toggle(filter.categories, c) })}>
              {c} ({n})
            </Chip>
          ))}
        </div>
      )}
      {tags.length > 0 && (
        <div role="group" aria-label="Tags" className="flex flex-wrap items-center gap-1.5">
          <span className="text-xs text-ink-muted">Tags</span>
          {shownTags.map(([t, n]) => (
            <Chip key={t} on={filter.tags.includes(t)} onClick={() => set({ tags: toggle(filter.tags, t) })}>
              {t} ({n})
            </Chip>
          ))}
          {tags.length > TOP_TAGS && (
            <Button variant="ghost" size="sm" onClick={() => setAllTags(!allTags)}>
              {allTags ? "fewer tags" : `${tags.length - TOP_TAGS} more tags`}
            </Button>
          )}
        </div>
      )}
      {result.total === 0 ? <EmptyState>Nothing matches these filters.</EmptyState> : children(result.items)}
      {result.total > PAGE_SIZE && (
        <div className="flex items-center justify-between py-1 text-sm">
          <span role="status" className="tabular-nums text-ink-muted">
            Showing {(result.page - 1) * PAGE_SIZE + 1} to {Math.min(result.page * PAGE_SIZE, result.total)} of {result.total}
          </span>
          <span className="flex gap-2">
            <Button disabled={result.page <= 1} onClick={() => set({ page: result.page - 1 })}>
              Previous
            </Button>
            <Button disabled={result.page >= result.pages} onClick={() => set({ page: result.page + 1 })}>
              Next
            </Button>
          </span>
        </div>
      )}
    </div>
  );
}
