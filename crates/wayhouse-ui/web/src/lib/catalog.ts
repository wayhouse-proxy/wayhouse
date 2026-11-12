// Search, filter, sort and page a registry catalog entirely in the browser, and keep the
// filter in the URL so a filtered view is linkable. Pure functions; the UI is CatalogBrowser.

export const PAGE_SIZE = 50;

/** What both sniffer and plugin catalog entries share. */
export interface CatalogItem {
  name: string;
  description: string;
  categories: string[];
  tags: string[];
  signed: boolean;
  /** YYYY-MM-DD */
  updated: string;
  /** Present only when the registry publishes stats; the "popular" sort needs it. */
  downloads?: number;
}

export type CatalogSort = "name" | "updated" | "popular";

export interface CatalogFilter {
  q: string;
  categories: string[];
  tags: string[];
  signedOnly: boolean;
  sort: CatalogSort;
  /** 1-based */
  page: number;
}

export function filterFromParams(params: URLSearchParams, canPopular: boolean): CatalogFilter {
  const sort = params.get("sort");
  const page = Number(params.get("page"));
  return {
    q: params.get("q") ?? "",
    categories: params.getAll("category"),
    tags: params.getAll("tag"),
    signedOnly: params.get("signed") === "1",
    sort: sort === "updated" || (sort === "popular" && canPopular) ? sort : "name",
    page: Number.isInteger(page) && page > 1 ? page : 1,
  };
}

/** `params` with this filter's keys replaced; any other key (the page's own) is kept. */
export function filterToParams(f: CatalogFilter, params: URLSearchParams): URLSearchParams {
  const out = new URLSearchParams(params);
  for (const k of ["q", "category", "tag", "signed", "sort", "page"]) out.delete(k);
  if (f.q) out.set("q", f.q);
  f.categories.forEach((c) => out.append("category", c));
  f.tags.forEach((t) => out.append("tag", t));
  if (f.signedOnly) out.set("signed", "1");
  if (f.sort !== "name") out.set("sort", f.sort);
  if (f.page > 1) out.set("page", String(f.page));
  return out;
}

export interface CatalogPage<T> {
  items: T[];
  total: number;
  page: number;
  pages: number;
}

export function applyFilter<T extends CatalogItem>(all: T[], f: CatalogFilter): CatalogPage<T> {
  const q = f.q.trim().toLowerCase();
  const hits = all.filter(
    (i) =>
      (!q || [i.name, i.description, ...i.tags].some((s) => s.toLowerCase().includes(q))) &&
      (f.categories.length === 0 || i.categories.some((c) => f.categories.includes(c))) &&
      (f.tags.length === 0 || i.tags.some((t) => f.tags.includes(t))) &&
      (!f.signedOnly || i.signed),
  );
  const byName = (a: T, b: T) => a.name.localeCompare(b.name);
  const order: Record<CatalogSort, (a: T, b: T) => number> = {
    name: byName,
    updated: (a, b) => b.updated.localeCompare(a.updated) || byName(a, b),
    popular: (a, b) => (b.downloads ?? 0) - (a.downloads ?? 0) || byName(a, b),
  };
  hits.sort(order[f.sort]);
  const pages = Math.max(1, Math.ceil(hits.length / PAGE_SIZE));
  const page = Math.min(f.page, pages);
  return { items: hits.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE), total: hits.length, page, pages };
}

/** Category and tag values with how many entries carry them, most used first. */
export function facets(all: CatalogItem[]): { categories: [string, number][]; tags: [string, number][] } {
  const count = (pick: (i: CatalogItem) => string[]) => {
    const m = new Map<string, number>();
    all.forEach((i) => pick(i).forEach((v) => m.set(v, (m.get(v) ?? 0) + 1)));
    return [...m].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  };
  return { categories: count((i) => i.categories), tags: count((i) => i.tags) };
}
