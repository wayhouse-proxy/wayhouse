import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import { effective, THEME_TOKENS, type Theme } from "./builtin";

/** "system", "light", "dark", or "custom:<name>" for a saved theme. */
export type ThemeChoice = string;

const KEY = "wayhouse.theme";

function stored(): ThemeChoice {
  try {
    return localStorage.getItem(KEY) ?? "system";
  } catch {
    return "system";
  }
}

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
  choice: ThemeChoice;
  setChoice: (c: ThemeChoice) => void;
  /** Applies a theme for this session only, without remembering it. Pass null to stop previewing. */
  preview: (t: Theme | null) => void;
}

const ThemeCtx = createContext<Ctx>({ choice: "system", setChoice: () => {}, preview: () => {} });
export const useTheme = () => useContext(ThemeCtx);

export function ThemeProvider({ themes = [], children }: { themes?: Theme[]; children: React.ReactNode }) {
  const [choice, setChoiceState] = useState(stored);
  const [previewing, preview] = useState<Theme | null>(null);

  useEffect(() => {
    if (previewing) applyTheme(document.documentElement, "custom:preview", [{ ...previewing, name: "preview" }]);
    else applyTheme(document.documentElement, choice, themes);
  }, [choice, themes, previewing]);

  const setChoice = useCallback((c: ThemeChoice) => {
    setChoiceState(c);
    try {
      localStorage.setItem(KEY, c);
    } catch {
      /* private window or blocked storage: the choice lasts for this page view */
    }
  }, []);

  const value = useMemo(() => ({ choice, setChoice, preview }), [choice, setChoice]);
  return <ThemeCtx.Provider value={value}>{children}</ThemeCtx.Provider>;
}
