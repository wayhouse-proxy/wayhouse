/** The aggregator's `GET /fleet/nodes` shapes and the URL state of the fleet page (docs/06-operations-observability.md). */

export type NodeState = "ok" | "degraded" | "stale";
export const NODE_STATES: NodeState[] = ["ok", "degraded", "stale"];
export const SORT_KEYS = ["instance", "state", "region", "sessions", "version"] as const;
export type SortKey = (typeof SORT_KEYS)[number];

export interface FleetNode {
  instance: string;
  state: NodeState;
  last_seen_ms_ago: number;
  region?: string;
  group?: string;
  tags: Record<string, string>;
  version: string;
  protocol: string;
  skew: "none" | "within-window" | "outside-window";
  sessions: { tcp: number; udp: number };
  backends: number;
  unhealthy_backends: number;
}

export interface Rollup {
  nodes: number;
  degraded: number;
  stale: number;
  sessions: number;
}

export interface NodePage {
  items: FleetNode[];
  total: number;
  page: number;
  size: number;
  rollup: Rollup;
  groups?: (Rollup & { region: string | null })[];
}

/** What the filter selects, as `POST /fleet/nodes/resolve` answers. */
export interface Resolved {
  count: number;
  instances: string[];
}

export interface Filter {
  q: string;
  state: NodeState | "";
  region: string;
  /** `key` or `key=value`; every one must match. */
  tags: string[];
}

export interface NodeQuery extends Filter {
  page: number;
  size: number;
  sort: SortKey;
  dir: "asc" | "desc";
  /** The instance open in the inspector. */
  sel: string;
}

const SIZES = [25, 100];
const positive = (s: string | null) => {
  const n = Number(s);
  return Number.isInteger(n) && n > 0 ? n : null;
};

export function parseNodeQuery(p: URLSearchParams): NodeQuery {
  const state = p.get("state") as NodeState;
  const size = positive(p.get("size"));
  const sort = p.get("sort") as SortKey;
  return {
    q: p.get("q") ?? "",
    state: NODE_STATES.includes(state) ? state : "",
    region: p.get("region") ?? "",
    tags: p.getAll("tag").filter(Boolean),
    page: positive(p.get("page")) ?? 1,
    size: size && SIZES.includes(size) ? size : 25,
    sort: SORT_KEYS.includes(sort) ? sort : "instance",
    dir: p.get("dir") === "desc" ? "desc" : "asc",
    sel: p.get("sel") ?? "",
  };
}

/** The filter part only: what `resolve` takes and what a saved filter keeps. */
export function filterQuery(f: Filter): string {
  const p = new URLSearchParams();
  if (f.q) p.set("q", f.q);
  if (f.state) p.set("state", f.state);
  if (f.region) p.set("region", f.region);
  f.tags.forEach((t) => p.append("tag", t));
  return p.toString();
}

/** The page URL's search string; defaults are left out so a fresh page has a clean URL. */
export function nodeQuery(q: NodeQuery): string {
  const p = new URLSearchParams(filterQuery(q));
  if (q.page > 1) p.set("page", String(q.page));
  if (q.size !== 25) p.set("size", String(q.size));
  if (q.sort !== "instance") p.set("sort", q.sort);
  if (q.dir !== "asc") p.set("dir", q.dir);
  if (q.sel) p.set("sel", q.sel);
  return p.toString();
}

export const filterOf = ({ q, state, region, tags }: Filter): Filter => ({ q, state, region, tags });

/** A saved filter as the controller stores it (docs/ui-data.md): a name and the filter fields. */
export interface SavedFilter extends Partial<Filter> {
  name: string;
}
export const toSavedFilter = (name: string, f: Filter): SavedFilter => {
  const rec: SavedFilter = { name };
  if (f.q) rec.q = f.q;
  if (f.state) rec.state = f.state;
  if (f.region) rec.region = f.region;
  if (f.tags.length) rec.tags = f.tags;
  return rec;
};
/** Fills what a stored record leaves out (a record from an older or hand-written save). */
export const savedFilter = (r: SavedFilter): SavedFilter & Filter => ({
  name: r.name,
  q: r.q ?? "",
  state: NODE_STATES.includes(r.state as NodeState) ? (r.state as NodeState) : "",
  region: r.region ?? "",
  tags: Array.isArray(r.tags) ? r.tags : [],
});
