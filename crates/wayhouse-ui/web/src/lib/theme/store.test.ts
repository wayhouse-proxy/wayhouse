import { describe, expect, it } from "vitest";
import { exportTheme, nameProblem, parseTheme, withToken } from "./store";

describe("nameProblem", () => {
  it("applies the controller's rules", () => {
    expect(nameProblem("Night shift")).toBeNull();
    expect(nameProblem(" ")).toMatch(/name/);
    expect(nameProblem("a/b")).toMatch(/cannot contain/);
    expect(nameProblem("x".repeat(65))).toMatch(/64/);
    expect(nameProblem("dark")).toMatch(/built-in/);
    expect(nameProblem("Night", ["Night"])).toMatch(/already exists/);
  });
});

describe("parseTheme", () => {
  it("round-trips an export", () => {
    const t = { name: "Night", base: "dark" as const, tokens: { select: "#7fb0ff" } };
    expect(parseTheme(exportTheme(t))).toEqual(t);
  });
  it("explains what is wrong", () => {
    expect(() => parseTheme("{")).toThrow(/Not valid JSON/);
    expect(() => parseTheme("[]")).toThrow(/JSON object/);
    expect(() => parseTheme('{"base":"x","tokens":{}}')).toThrow(/base/);
    expect(() => parseTheme('{"base":"dark"}')).toThrow(/tokens/);
  });
});

describe("withToken", () => {
  it("drops an override that equals the base", () => {
    const t = { name: "n", base: "light" as const, tokens: { ink: "#000000" } };
    expect(withToken(t, "ink", "#111111", "#222222").tokens).toEqual({ ink: "#111111" });
    expect(withToken(t, "ink", "#222222", "#222222").tokens).toEqual({});
  });
});
