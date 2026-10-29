export type Path = (string | number)[];

export function get(value: any, path: Path): any {
  return path.reduce((v, k) => (v === null || typeof v !== "object" ? undefined : v[k]), value);
}

/** A copy of `value` with `path` set to `next`, creating the objects on the way. */
export function set(value: any, path: Path, next: any): any {
  if (path.length === 0) return next;
  const [k, ...rest] = path;
  const base = value !== null && typeof value === "object" ? value : typeof k === "number" ? [] : {};
  const copy: any = Array.isArray(base) ? [...base] : { ...base };
  copy[k] = set(base[k], rest, next);
  return copy;
}
