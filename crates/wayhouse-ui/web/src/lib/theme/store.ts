import type { StoredTheme } from "../../api";
import { THEME_TOKENS } from "./builtin";

const RESERVED = ["light", "dark", "system"];

/** The controller's rule for a theme name (docs/ui-data.md): 1 to 64 characters, no `/` or control characters, not a built-in. */
export function nameProblem(name: string, taken: string[] = []): string | null {
  if (!name.trim()) return "Give the theme a name.";
  if (name.length > 64) return "A name has at most 64 characters.";
  // eslint-disable-next-line no-control-regex
  if (name.includes("/") || /[\u0000-\u001f\u007f]/.test(name)) return "A name cannot contain / or control characters.";
  if (RESERVED.includes(name.toLowerCase())) return `"${name}" is a built-in theme; pick another name.`;
  if (taken.includes(name)) return `A theme named "${name}" already exists.`;
  return null;
}

/** Reads an imported theme: the same JSON the controller stores and the export gives. Throws a sentence the person can act on. */
export function parseTheme(text: string): StoredTheme {
  let raw: any;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    throw new Error(`Not valid JSON: ${(e as Error).message}`);
  }
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) throw new Error("A theme is a JSON object with name, base and tokens.");
  if (raw.base !== "light" && raw.base !== "dark") throw new Error('"base" must be "light" or "dark".');
  if (typeof raw.tokens !== "object" || raw.tokens === null || Array.isArray(raw.tokens)) throw new Error('"tokens" must be an object of #rrggbb colours.');
  const tokens: Record<string, string> = {};
  for (const [k, v] of Object.entries(raw.tokens)) if (typeof v === "string") tokens[k] = v;
  return { name: typeof raw.name === "string" ? raw.name : "", base: raw.base, tokens };
}

export const exportTheme = (t: StoredTheme) => JSON.stringify({ name: t.name, base: t.base, tokens: t.tokens }, null, 2);

/** The tokens a theme sets that differ from the base's value; setting a token back to the base's value drops the override. */
export function withToken(t: StoredTheme, token: string, value: string, baseValue: string): StoredTheme {
  const tokens = { ...t.tokens };
  if (value === baseValue) delete tokens[token];
  else tokens[token] = value;
  return { ...t, tokens };
}

export const TOKEN_GROUPS: { label: string; tokens: string[] }[] = [
  { label: "Ground and text", tokens: ["bg", "surface", "surface-sunken", "ink", "ink-muted", "line", "control-border"] },
  { label: "Navigation rail", tokens: ["rail", "rail-ink", "rail-ink-muted", "rail-hover"] },
  { label: "Interaction", tokens: ["select", "select-bg", "primary", "primary-ink"] },
  { label: "State colours", tokens: ["good", "warn", "bad"] },
  { label: "Data series", tokens: ["series-1", "series-2", "series-3", "series-4", "series-5"] },
];
// A token added to THEME_TOKENS without a group would silently vanish from the editor.
if (TOKEN_GROUPS.flatMap((g) => g.tokens).sort().join() !== [...THEME_TOKENS].sort().join()) throw new Error("TOKEN_GROUPS and THEME_TOKENS differ");
