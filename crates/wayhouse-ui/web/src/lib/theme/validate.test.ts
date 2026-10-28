import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
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

  it("the media-query dark block equals the data-theme dark block", () => {
    const grab = (re: RegExp) => [...re.exec(tokensCss)![1].matchAll(/--[\w-]+:\s*#[0-9a-f]{6}/gi)].map((m) => m[0]);
    expect(grab(/:root:not\(\[data-theme\]\) \{([\s\S]*?)\n  \}/)).toEqual(grab(/:root\[data-theme="dark"\] \{([\s\S]*?)\n\}/));
  });
});
