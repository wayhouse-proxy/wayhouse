import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import { effective, THEME_TOKENS, type Theme } from "./builtin";

/** "system", "light", "dark", or "custom:<name>" for a saved theme. */
export type ThemeChoice = string;

const KEY = "wayhouse.theme";

function stored(): ThemeChoice | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

/** The deployment default as a choice: a built-in or system by name, otherwise a saved theme. */
const asChoice = (name: string | undefined, themes: Theme[]): ThemeChoice =>
  !name ? "system" : ["system", "light", "dark"].includes(name) ? name : themes.some((t) => t.name === name) ? `custom:${name}` : "system";

/**
 * Applies a choice to the root element: system removes `data-theme`, light and dark set it,
 * a custom theme sets `data-theme="custom"` plus every token as a variable (its base's values
 * under the overrides) and `color-scheme` from its base.
 */
export function applyTheme(root: HTMLElement, choice: ThemeChoice, themes: Theme[] = []) {
  for (const t of THEME_TOKENS) root.style.removeProperty(`--${t}`);
  root.style.removeProperty("color-scheme");
  const custom = choice.startsWith("custom:") ? themes.find((t) => t.name === choice.slice(7)) : undefined;
  if (custom) {
    root.dataset.theme = "custom";
    root.style.colorScheme = custom.base;
    for (const [k, v] of Object.entries(effective(custom))) root.style.setProperty(`--${k}`, v);
  } else if (choice === "light" || choice === "dark") {
    root.dataset.theme = choice;
  } else {
    delete root.dataset.theme; // a deleted custom theme falls back to the system setting
  }
}

interface Ctx {
  /** What is applied now: this browser's own choice, else the deployment default, else the system setting. */
  choice: ThemeChoice;
  /** Whether the browser has its own choice (otherwise the deployment default applies). */
  own: boolean;
  /** Remember a choice in this browser; null forgets it, so the deployment default applies again. */
  setChoice: (c: ThemeChoice | null) => void;
  /** The saved themes and the deployment default, as the controller stores them. */
  themes: Theme[];
  defaultTheme?: string;
  setSaved: (themes: Theme[], defaultTheme?: string) => void;
  /** Applies a theme for this session only, without remembering it. Pass null to stop previewing. */
  preview: (t: Theme | null) => void;
}

const ThemeCtx = createContext<Ctx>({ choice: "system", own: false, setChoice: () => {}, themes: [], setSaved: () => {}, preview: () => {} });
export const useTheme = () => useContext(ThemeCtx);

export function ThemeProvider({ themes: initial = [], children }: { themes?: Theme[]; children: React.ReactNode }) {
  const [own, setOwn] = useState(stored);
  const [saved, setSavedState] = useState<{ themes: Theme[]; defaultTheme?: string }>({ themes: initial });
  const [previewing, preview] = useState<Theme | null>(null);
  const { themes, defaultTheme } = saved;
  const choice = own ?? asChoice(defaultTheme, themes);

  useEffect(() => {
    if (previewing) applyTheme(document.documentElement, "custom:preview", [{ ...previewing, name: "preview" }]);
    else applyTheme(document.documentElement, choice, themes);
  }, [choice, themes, previewing]);

  const setChoice = useCallback((c: ThemeChoice | null) => {
    setOwn(c);
    try {
      if (c === null) localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, c);
    } catch {
      /* private window or blocked storage: the choice lasts for this page view */
    }
  }, []);

  const setSaved = useCallback((t: Theme[], d?: string) => setSavedState({ themes: t, defaultTheme: d }), []);
  const value = useMemo(
    () => ({ choice, own: own !== null, setChoice, themes, defaultTheme, setSaved, preview }),
    [choice, own, setChoice, themes, defaultTheme, setSaved],
  );
  return <ThemeCtx.Provider value={value}>{children}</ThemeCtx.Provider>;
}
