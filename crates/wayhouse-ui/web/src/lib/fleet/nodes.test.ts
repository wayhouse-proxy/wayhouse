import { describe, expect, it } from "vitest";
import { filterOf, filterQuery, nodeQuery, parseNodeQuery, savedFilter, toSavedFilter } from "./nodes";

describe("fleet node query in the URL", () => {
  it("round-trips filters, paging and sort, leaving defaults out", () => {
    const p = new URLSearchParams("q=fra&state=degraded&region=eu&tag=env%3Dprod&tag=gpu&page=3&size=100&sort=sessions&dir=desc&sel=fra-1");
    const q = parseNodeQuery(p);
    expect(q).toMatchObject({ q: "fra", state: "degraded", region: "eu", tags: ["env=prod", "gpu"], page: 3, size: 100, sort: "sessions", dir: "desc", sel: "fra-1" });
    expect(parseNodeQuery(new URLSearchParams(nodeQuery(q)))).toEqual(q);
    expect(nodeQuery(parseNodeQuery(new URLSearchParams()))).toBe("");
  });

  it("falls back to defaults for junk", () => {
    const q = parseNodeQuery(new URLSearchParams("state=sad&page=-4&size=7&sort=nope&dir=up"));
    expect(q).toMatchObject({ state: "", page: 1, size: 25, sort: "instance", dir: "asc" });
  });

  it("sends only the filter to resolve, never paging or the selection", () => {
    const q = parseNodeQuery(new URLSearchParams("q=a&tag=x%3D1&page=2&sel=n1"));
    expect(filterQuery(q)).toBe("q=a&tag=x%3D1");
  });

  it("saves and restores a filter without paging", () => {
    const q = parseNodeQuery(new URLSearchParams("q=a&state=stale&region=eu&tag=x%3D1&page=2"));
    const rec = toSavedFilter("stale eu", q);
    expect(rec).toEqual({ name: "stale eu", q: "a", state: "stale", region: "eu", tags: ["x=1"] });
    expect(filterOf(savedFilter(rec))).toEqual({ q: "a", state: "stale", region: "eu", tags: ["x=1"] });
    expect(savedFilter({ name: "bare" })).toEqual({ name: "bare", q: "", state: "", region: "", tags: [] });
  });
});
