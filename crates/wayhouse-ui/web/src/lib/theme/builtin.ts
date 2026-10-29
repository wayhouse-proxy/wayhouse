// Values of the two built-in themes. docs/design-system/tokens.css is the
// source; validate.test.ts fails when this file and the css disagree.

export type Tokens = Record<string, string>;
export type Base = "light" | "dark";
export interface Theme {
  name?: string;
  base: Base;
  tokens: Tokens;
}

export const THEME_TOKENS = [
  "bg", "surface", "surface-sunken", "ink", "ink-muted", "line", "control-border",
  "rail", "rail-ink", "rail-ink-muted", "rail-hover",
  "select", "select-bg", "primary", "primary-ink",
  "good", "warn", "bad",
  "series-1", "series-2", "series-3", "series-4", "series-5",
] as const;

export const BUILTIN: Record<Base, Tokens> = {
  light: {
    bg: "#f3f1ee", surface: "#fbfaf8", "surface-sunken": "#ebe8e3", ink: "#1d1b18",
    "ink-muted": "#6a655d", line: "#dcd8d1", "control-border": "#857f75",
    rail: "#ebe8e3", "rail-ink": "#1d1b18", "rail-ink-muted": "#625d55", "rail-hover": "#dedad3",
    select: "#2b5cd6", "select-bg": "#e3eafb", primary: "#1d1b18", "primary-ink": "#fbfaf8",
    good: "#1b7654", warn: "#b87400", bad: "#c40e3b",
    "series-1": "#2b5ff3", "series-2": "#219a77", "series-3": "#984f85", "series-4": "#5a1fad", "series-5": "#8e0b62",
  },
  dark: {
    bg: "#16171a", surface: "#1d1f23", "surface-sunken": "#202226", ink: "#ecebe7",
    "ink-muted": "#9a968d", line: "#2e3035", "control-border": "#7a7d85",
    rail: "#0e0f11", "rail-ink": "#e9e7e2", "rail-ink-muted": "#96948f", "rail-hover": "#22252a",
    select: "#8aa9ff", "select-bg": "#232b44", primary: "#ecebe7", "primary-ink": "#16171a",
    good: "#4bbfa6", warn: "#fbc43b", bad: "#e0645c",
    "series-1": "#6e8dff", "series-2": "#5ce1b4", "series-3": "#c1bedc", "series-4": "#d7c143", "series-5": "#98868e",
  },
};

/** The effective tokens of a theme: the base's values with the overrides on top. */
export const effective = (t: Theme): Tokens => ({ ...BUILTIN[t.base], ...t.tokens });
