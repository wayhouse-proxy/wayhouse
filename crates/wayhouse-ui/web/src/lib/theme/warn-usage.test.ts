import { readdirSync, readFileSync, statSync } from "node:fs";
import { describe, expect, it } from "vitest";

// warn is held to the 3:1 graphic ratio (themes.md check 6), so it may colour marks and borders but never text.
const files = (dir: string): string[] =>
  readdirSync(dir).flatMap((n) => {
    const p = `${dir}/${n}`;
    return statSync(p).isDirectory() ? files(p) : /\.tsx$/.test(n) && !/\.test\./.test(n) ? [p] : [];
  });

describe("warn colour", () => {
  it("is never used as a text colour, only on icons", () => {
    const bad = files(`${import.meta.dirname}/../..`).flatMap((f) =>
      readFileSync(f, "utf8")
        .split("\n")
        .filter((l) => /\btext-warn\b/.test(l) && !/<AlertTriangle aria-hidden/.test(l))
        .map((l) => `${f}: ${l.trim()}`),
    );
    expect(bad).toEqual([]);
  });
});
