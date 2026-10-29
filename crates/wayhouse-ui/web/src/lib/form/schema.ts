import type { RJSFSchema } from "@rjsf/utils";

type S = Record<string, any>;

/**
 * Reshapes what `schemars` emits into what the form renders well, without changing meaning:
 * "or null" becomes an optional field, a `oneOf` of string constants becomes one `enum`, and a
 * port that is an integer or a string becomes a string field marked `x-wayhouse-int-or-string`
 * (see `coerce`), and `default` moves to `x-wayhouse-default`. Every `x-wayhouse-*` annotation is kept.
 */
export function simplify(node: any): any {
  if (Array.isArray(node)) return node.map(simplify);
  if (typeof node !== "object" || node === null) return node;
  let n: S = { ...node };

  if (Array.isArray(n.anyOf)) {
    const rest = n.anyOf.filter((x: S) => x?.type !== "null");
    const { anyOf, ...siblings } = n;
    if (rest.length === 1) n = { ...rest[0], ...siblings };
    else if (rest.every((x: S) => x.type === "integer" || x.type === "string"))
      n = { ...siblings, type: "string", "x-wayhouse-int-or-string": true };
  }
  if (Array.isArray(n.oneOf) && n.oneOf.every((x: S) => x?.type === "string" && ("const" in x || x.enum))) {
    const { oneOf, ...siblings } = n;
    n = { ...siblings, type: "string", enum: oneOf.flatMap((x: S) => x.enum ?? [x.const]) };
  }
  if (Array.isArray(n.type)) {
    const types = n.type.filter((t: string) => t !== "null");
    n.type = types.length === 1 ? types[0] : types;
  }
  // A default is shown as a placeholder, never written into the form data (the file says only what the operator set).
  if ("default" in n) {
    if (n.default !== null) n["x-wayhouse-default"] = n.default;
    delete n.default;
  }

  for (const k of Object.keys(n)) {
    if (k === "properties" || k === "$defs") n[k] = Object.fromEntries(Object.entries(n[k]).map(([p, v]) => [p, simplify(v)]));
    else if (k === "items" || k === "additionalProperties" || k === "anyOf" || k === "oneOf" || k === "allOf") n[k] = simplify(n[k]);
  }
  return n;
}

export const sinceOf = (schema: S): number | undefined => schema["x-wayhouse-since"];

const UNIT_SUFFIXES: [string, string][] = [
  ["_per_sec", "per second"],
  ["_ms", "milliseconds"],
  ["_sec", "seconds"],
  ["_bytes", "bytes"],
];

/** The unit shown beside a number: `x-wayhouse-unit` when the schema says it, else read from the field name. */
export function unitFor(name: string, schema: S): string | undefined {
  return schema["x-wayhouse-unit"] ?? UNIT_SUFFIXES.find(([suffix]) => name.endsWith(suffix))?.[1];
}

/** Resolves a `$ref` into the schema's `$defs`; other nodes come back as they are. */
export function resolve(root: S, node: S): S {
  const ref = node?.$ref as string | undefined;
  if (!ref?.startsWith("#/$defs/")) return node;
  const { $ref, ...siblings } = node;
  return { ...root.$defs[ref.slice(8)], ...siblings };
}

/** The sub-schema for `path` (names and array indices), or undefined when the schema has no such place. */
export function subschema(root: S, path: (string | number)[]): S | undefined {
  let node: S | undefined = resolve(root, root);
  for (const p of path) {
    if (!node) return undefined;
    node = resolve(root, node);
    node = typeof p === "number" ? node.items : node.properties?.[p] ?? (typeof node.additionalProperties === "object" ? node.additionalProperties : undefined);
    if (node) node = resolve(root, node);
  }
  return node;
}

/**
 * Writes a number where the schema says a field is an integer or a string and the text is digits only,
 * so `25565` stays `25565` in the file and `25565-25570` stays a string.
 */
export function coerce(root: S, schema: S, value: any): any {
  const node = resolve(root, schema);
  if (node["x-wayhouse-int-or-string"] && typeof value === "string" && /^\d+$/.test(value)) return Number(value);
  if (Array.isArray(value) && node.items) return value.map((v) => coerce(root, node.items, v));
  if (value && typeof value === "object" && !Array.isArray(value) && node.properties)
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, node.properties[k] ? coerce(root, node.properties[k], v) : v]));
  return value;
}

export type { RJSFSchema };
