// The ten theme checks of docs/design-system/themes.md. The controller runs a
// Rust port of this on save (B3); fixtures.json holds the cases both must agree on.
import { contrast, deltaE00, parseHex, simulate, type Cvd, type Rgb } from "./color";
import { effective, THEME_TOKENS, type Theme } from "./builtin";

/** Smallest CIEDE2000 difference between two colours that must stay apart (check 8, 9, 10). */
export const MIN_DELTA_E = 10;
export const TEXT_RATIO = 4.5;
export const GRAPHIC_RATIO = 3;
const VISIONS: (Cvd | null)[] = [null, "deuteranopia", "protanopia", "tritanopia"];
const GROUNDS = ["bg", "surface", "surface-sunken"];
const STATES = ["good", "warn", "bad"];
const SERIES = ["series-1", "series-2", "series-3", "series-4", "series-5"];

export interface Failure {
  /** 1..10 as in themes.md; 0 for a malformed theme (unknown token, not #rrggbb). */
  check: number;
  pair: [string, string];
  value: number;
  needed: number;
  message: string;
}

export function validateTheme(theme: Theme): Failure[] {
  const out: Failure[] = [];
  const known = new Set<string>(THEME_TOKENS);
  for (const [k, v] of Object.entries(theme.tokens)) {
    if (!known.has(k)) out.push({ check: 0, pair: [k, k], value: 0, needed: 0, message: `Unknown token "${k}".` });
    else if (!parseHex(v)) out.push({ check: 0, pair: [k, k], value: 0, needed: 0, message: `${k} must be a #rrggbb colour, got "${v}".` });
  }
  if (out.length) return out;

  const t = effective(theme);
  const c = (n: string) => parseHex(t[n])!;
  const ratio = (check: number, fg: string, bg: string, needed: number) => {
    const value = contrast(c(fg), c(bg));
    if (value < needed)
      out.push({ check, pair: [fg, bg], value, needed, message: `${fg} on ${bg} is ${value.toFixed(2)}:1, needs ${needed}:1.` });
  };
  const apart = (check: number, a: string, b: string, visions: (Cvd | null)[]) => {
    for (const v of visions) {
      const [x, y]: Rgb[] = [c(a), c(b)].map((rgb) => (v ? simulate(rgb, v) : rgb));
      const value = deltaE00(x, y);
      if (value < MIN_DELTA_E) {
        out.push({ check, pair: [a, b], value, needed: MIN_DELTA_E,
          message: `${a} and ${b} differ by ${value.toFixed(1)} (${v ?? "normal vision"}), need ${MIN_DELTA_E}.` });
        return;
      }
    }
  };
  const pairs = (names: string[]) => names.flatMap((a, i) => names.slice(i + 1).map((b) => [a, b] as const));

  for (const g of GROUNDS) ratio(1, "ink", g, TEXT_RATIO);
  for (const g of GROUNDS) ratio(2, "ink-muted", g, TEXT_RATIO);
  for (const f of ["rail-ink", "rail-ink-muted"]) for (const g of ["rail", "rail-hover"]) ratio(3, f, g, TEXT_RATIO);
  ratio(4, "select", "surface", TEXT_RATIO);
  ratio(4, "ink", "select-bg", TEXT_RATIO);
  ratio(5, "primary-ink", "primary", TEXT_RATIO);
  for (const s of STATES) for (const g of GROUNDS) ratio(6, s, g, TEXT_RATIO);
  for (const g of ["surface", "bg"]) ratio(7, "control-border", g, GRAPHIC_RATIO);
  for (const [a, b] of pairs(STATES)) apart(8, a, b, VISIONS);
  for (const s of SERIES) ratio(9, s, "surface", GRAPHIC_RATIO);
  for (const [a, b] of pairs(SERIES)) apart(9, a, b, VISIONS);
  for (const s of STATES) apart(10, "select", s, [null]);
  return out;
}

export const failedChecks = (f: Failure[]) => [...new Set(f.map((x) => x.check))].sort((a, b) => a - b);
