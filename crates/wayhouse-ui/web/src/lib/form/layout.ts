import defaultLayout from "./default-layout.json";

/**
 * The optional layout of generated forms (docs/ui-form-layout.md). It groups, orders and explains fields and
 * never changes what the YAML says. `fields` is keyed by dotted path with `*` for any list index or map key.
 */
export interface FieldLayout {
  label?: string;
  help?: string;
  /** Section title inside the parent object. */
  group?: string;
  /** Smaller first; fields without one keep the schema's order after the ordered ones. */
  order?: number;
  /** "secret", "lines", "select", "switch", "number" or "text"; default is picked from the schema type. */
  widget?: "secret" | "lines" | "select" | "switch" | "number" | "text";
  unit?: string;
  /** Shown only when Show advanced is on. */
  advanced?: boolean;
  /** Which settings page shows it (A3b pages are named in the default layout). */
  page?: string;
}

export interface Layout {
  version: 1;
  fields: Record<string, FieldLayout>;
}

export const DEFAULT_LAYOUT = defaultLayout as Layout;

/** `listeners.0.bind` is looked up as `listeners.*.bind`. */
export const patternOf = (path: (string | number)[]) => path.map((p) => (typeof p === "number" ? "*" : p)).join(".");

/** What an override may set. The controller stores exactly these (docs/ui-data.md); `label` and `unit` belong to the shipped default only. */
const FIELD_KEYS = new Set(["help", "group", "order", "widget", "advanced", "page"]);
const WIDGETS = new Set(["secret", "lines", "select", "switch", "number", "text"]);

/** An override key is a JSON Pointer into the config document (`/pools/0/targets`; a number or a star stands for any list item); the dotted form of the shipped default is accepted too. */
export function patternFromKey(key: string): string {
  if (!key.startsWith("/")) return key;
  return key
    .slice(1)
    .split("/")
    .map((s) => s.replace(/~1/g, "/").replace(/~0/g, "~"))
    .map((s) => (/^\d+$/.test(s) ? "*" : s))
    .join(".");
}

export interface LayoutResult {
  layout: Layout;
  /** What was ignored or wrong, in words; shown as a notice, never blocks editing. */
  warnings: string[];
}

/**
 * Checks an operator's override. Unknown paths (checked against `knownPaths`) and unknown keys are dropped with a
 * warning; a wrong shape is an error for the editor that submits it (`error`), and the default layout stays in use.
 */
export function parseOverride(text: string, knownPaths: Set<string>): LayoutResult & { error?: string } {
  let raw: any;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    return { layout: { version: 1, fields: {} }, warnings: [], error: `The layout is not valid JSON: ${(e as Error).message}` };
  }
  if (typeof raw !== "object" || raw === null || raw.version !== 1 || typeof raw.fields !== "object" || raw.fields === null || Array.isArray(raw.fields))
    return { layout: { version: 1, fields: {} }, warnings: [], error: 'The layout must be an object like {"version": 1, "fields": {...}}.' };
  const fields: Record<string, FieldLayout> = {};
  const warnings: string[] = [];
  for (const [key, f] of Object.entries<any>(raw.fields)) {
    const path = key;
    if (!knownPaths.has(patternFromKey(key))) {
      warnings.push(`"${path}" is not a field of the configuration; ignored.`);
      continue;
    }
    if (typeof f !== "object" || f === null) {
      warnings.push(`"${path}" must be an object; ignored.`);
      continue;
    }
    const clean: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(f)) {
      if (!FIELD_KEYS.has(k)) warnings.push(`"${path}": unknown key "${k}"; ignored.`);
      else if (k === "widget" && !WIDGETS.has(v as string)) warnings.push(`"${path}": unknown widget "${String(v)}"; ignored.`);
      else if ((k === "order" && typeof v !== "number") || (k === "advanced" && typeof v !== "boolean") || (!["order", "advanced"].includes(k) && typeof v !== "string"))
        warnings.push(`"${path}": "${k}" has the wrong type; ignored.`);
      else clean[k] = v;
    }
    fields[patternFromKey(key)] = clean as FieldLayout;
  }
  return { layout: { version: 1, fields }, warnings };
}

/** The effective layout: the override's keys win field by field over the shipped default. */
export function mergeLayout(base: Layout, override?: Layout | null): Layout {
  if (!override) return base;
  const fields = { ...base.fields };
  for (const [k, v] of Object.entries(override.fields)) fields[k] = { ...fields[k], ...v };
  return { version: 1, fields };
}

/** Every field path of the schema, in `patternOf` form, for checking an override against. */
export function knownPaths(root: Record<string, any>): Set<string> {
  const out = new Set<string>();
  const seen = new Set<string>();
  const walk = (node: any, prefix: string[]) => {
    if (!node || typeof node !== "object") return;
    if (node.$ref?.startsWith("#/$defs/")) {
      const name = node.$ref.slice(8);
      if (seen.has(`${prefix.join(".")}|${name}`) || prefix.length > 8) return;
      seen.add(`${prefix.join(".")}|${name}`);
      walk(root.$defs[name], prefix);
      return;
    }
    for (const [k, v] of Object.entries<any>(node.properties ?? {})) {
      const p = [...prefix, k];
      out.add(p.join("."));
      walk(v, p);
    }
    if (node.items) walk(node.items, [...prefix, "*"]);
  };
  walk(root, []);
  return out;
}
