import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useStable } from "./useStable";

describe("useStable", () => {
  it("keeps the same object while a new one has the same content, and takes a changed one", () => {
    const { result, rerender } = renderHook(({ v }) => useStable(v), { initialProps: { v: { a: 1, b: [1] } as { a: number; b: number[] } | undefined } });
    const first = result.current;
    rerender({ v: { a: 1, b: [1] } });
    expect(result.current).toBe(first);
    rerender({ v: { a: 2, b: [1] } });
    expect(result.current).not.toBe(first);
    expect(result.current).toEqual({ a: 2, b: [1] });
    rerender({ v: undefined });
    expect(result.current).toBeUndefined();
  });
});
