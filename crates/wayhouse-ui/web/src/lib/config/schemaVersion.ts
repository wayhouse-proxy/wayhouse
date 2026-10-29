import { diff, type Path } from "./patch";
import { simplify, sinceOf, subschema } from "../form/schema";

/**
 * The lowest `schema_version` a document needs once `before` becomes `after`: the highest
 * `x-wayhouse-since` on the path of any value the edit sets, including the keys inside a value
 * that is new as a whole. Deleting a value needs nothing.
 */
export function requiredSchemaVersion(schema: Record<string, any>, before: unknown, after: unknown): number {
  const root = simplify(schema);
  let need = 1;
  const visit = (path: Path, value: unknown) => {
    need = Math.max(need, sinceOf(subschema(root, path) ?? {}) ?? 1);
    if (value && typeof value === "object")
      for (const [k, v] of Object.entries(value)) visit([...path, Array.isArray(value) ? Number(k) : k], v);
  };
  for (const op of diff(before, after)) {
    if (op.kind !== "set") continue;
    for (let n = 1; n < op.path.length; n++) need = Math.max(need, sinceOf(subschema(root, op.path.slice(0, n)) ?? {}) ?? 1);
    visit(op.path, op.value);
  }
  return need;
}
