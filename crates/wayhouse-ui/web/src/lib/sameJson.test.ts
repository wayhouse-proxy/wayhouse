import { describe, expect, it } from "vitest";
import { sameJson } from "./sameJson";

describe("sameJson", () => {
  it("ignores key order at every depth", () => {
    expect(sameJson({ a: 1, b: { c: [1, { x: 1, y: 2 }] } }, { b: { c: [1, { y: 2, x: 1 }] }, a: 1 })).toBe(true);
  });
  it("sees a changed value, a missing key and a different array order", () => {
    expect(sameJson({ a: 1 }, { a: 2 })).toBe(false);
    expect(sameJson({ a: 1 }, { a: 1, b: 1 })).toBe(false);
    expect(sameJson([1, 2], [2, 1])).toBe(false);
  });
});
