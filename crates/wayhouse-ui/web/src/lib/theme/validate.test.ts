import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { parseHex } from "./color";
import fixtures from "./fixtures.json";
import { BUILTIN, THEME_TOKENS, type Theme } from "./builtin";
import { failedChecks, validateTheme } from "./validate";

const tokensCss = readFileSync(`${import.meta.dirname}/../../tokens.css`, "utf8");
const docsTokens = readFileSync(`${import.meta.dirname}/../../../../../../docs/design-system/tokens.css`, "utf8");

describe("theme validator", () => {
  it.each(fixtures)("$name", ({ theme, failedChecks: want }) => {
    expect(failedChecks(validateTheme(theme as Theme))).toEqual(want);
  });

  it("names the token pair and the ratio", () => {
    const [f] = validateTheme({ base: "light", tokens: { ink: "#bbbbbb" } });
    expect(f.pair).toEqual(["ink", "bg"]);
    expect(f.message).toMatch(/^ink on bg is \d\.\d\d:1, needs 4\.5:1\.$/);
  });
});

describe("built-in themes", () => {
  it("the web copy of tokens.css is the design system's file", () => {
    expect(tokensCss).toBe(docsTokens);
  });

  // The values in builtin.ts are what the validator checks, so they must be the ones the css ships.
  it.each(["light", "dark"] as const)("%s values match tokens.css", (base) => {
    const block =
      base === "light"
        ? /^:root \{([\s\S]*?)^\}/m.exec(tokensCss)![1]
        : /:root\[data-theme="dark"\] \{([\s\S]*?)\n\}/.exec(tokensCss)![1];
    const css = Object.fromEntries([...block.matchAll(/--([\w-]+):\s*(#[0-9a-f]{6})/gi)].map((m) => [m[1], m[2]]));
    for (const name of THEME_TOKENS) expect(css[name], name).toBe(BUILTIN[base][name]);
  });

  // A state colour must look like its convention: warn is amber (yellow-orange), never brown.
  it.each(["light", "dark"] as const)("%s warn is amber, not brown", (base) => {
    const [r, g, b] = parseHex(BUILTIN[base].warn)!;
    const [max, min] = [Math.max(r, g, b), Math.min(r, g, b)];
    const l = (max + min) / 2;
    const s = (max - min) / (1 - Math.abs(2 * l - 1));
    const h = (60 * (max === r ? ((g - b) / (max - min)) % 6 : max === g ? (b - r) / (max - min) + 2 : (r - g) / (max - min) + 4) + 360) % 360;
    expect(h).toBeGreaterThanOrEqual(35);
    expect(h).toBeLessThanOrEqual(55);
    expect(s).toBeGreaterThanOrEqual(0.8);
    expect(l).toBeGreaterThanOrEqual(0.33);
  });

  // No chart series in the muddy olive/brown band (hue 20-100 degrees at low lightness or saturation).
  it("light series avoid muddy olive and brown", () => {
    const base = "light";
    for (const n of ["series-1", "series-2", "series-3", "series-4", "series-5"]) {
      const [r, g, b] = parseHex(BUILTIN[base][n])!;
      const [max, min] = [Math.max(r, g, b), Math.min(r, g, b)];
      const h = max === min ? 0 : (60 * (max === r ? ((g - b) / (max - min)) % 6 : max === g ? (b - r) / (max - min) + 2 : (r - g) / (max - min) + 4) + 360) % 360;
      const l = (max + min) / 2;
      expect(h >= 20 && h <= 100 && l < 0.5, `${n} ${BUILTIN[base][n]}`).toBe(false);
    }
  });

  it("the media-query dark block equals the data-theme dark block", () => {
    const grab = (re: RegExp) => [...re.exec(tokensCss)![1].matchAll(/--[\w-]+:\s*#[0-9a-f]{6}/gi)].map((m) => m[0]);
    expect(grab(/:root:not\(\[data-theme\]\) \{([\s\S]*?)\n  \}/)).toEqual(grab(/:root\[data-theme="dark"\] \{([\s\S]*?)\n\}/));
  });
});
