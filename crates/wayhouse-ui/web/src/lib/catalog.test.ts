import { describe, expect, it } from "vitest";
import { applyFilter, facets, filterFromParams, filterToParams, PAGE_SIZE, type CatalogItem } from "./catalog";

const item = (name: string, o: Partial<CatalogItem> = {}): CatalogItem => ({
  name,
  description: `${name} sniffer`,
  categories: ["game"],
  tags: [],
  signed: true,
  updated: "2026-11-01",
  ...o,
});

const items = [
  item("minecraft", { tags: ["java-edition", "minecraft"], updated: "2026-11-12", downloads: 10 }),
  item("a2s", { tags: ["source"], updated: "2026-10-01", downloads: 99 }),
  item("wireguard", { categories: ["vpn"], tags: ["tunnel"], signed: false, updated: "2026-09-01" }),
  item("openvpn", { categories: ["vpn"], tags: ["tunnel", "legacy"], updated: "2026-08-01", description: "OpenVPN reset" }),
];
const base = filterFromParams(new URLSearchParams(), false);
const names = (f: Partial<typeof base>) => applyFilter(items, { ...base, ...f }).items.map((i) => i.name);

describe("catalog filter", () => {
  it("sorts by name by default and by recency or downloads on request", () => {
    expect(names({})).toEqual(["a2s", "minecraft", "openvpn", "wireguard"]);
    expect(names({ sort: "updated" })).toEqual(["minecraft", "a2s", "wireguard", "openvpn"]);
    expect(names({ sort: "popular" }).slice(0, 2)).toEqual(["a2s", "minecraft"]);
  });
  it("searches name, description and tags, case-insensitively", () => {
    expect(names({ q: "MINECRAFT" })).toEqual(["minecraft"]);
    expect(names({ q: "reset" })).toEqual(["openvpn"]);
    expect(names({ q: "legacy" })).toEqual(["openvpn"]);
  });
  it("ORs inside a group and ANDs across groups", () => {
    expect(names({ categories: ["game", "vpn"] })).toHaveLength(4);
    expect(names({ categories: ["vpn"], tags: ["tunnel", "source"] })).toEqual(["openvpn", "wireguard"]);
    expect(names({ categories: ["game"], tags: ["tunnel"] })).toEqual([]);
    expect(names({ signedOnly: true, categories: ["vpn"] })).toEqual(["openvpn"]);
  });
  it("pages by 50 and clamps a page that is out of range", () => {
    const many = Array.from({ length: PAGE_SIZE + 5 }, (_, i) => item(`s${String(i).padStart(3, "0")}`));
    expect(applyFilter(many, { ...base, page: 2 }).items).toHaveLength(5);
    const r = applyFilter(many, { ...base, page: 9 });
    expect([r.page, r.pages, r.total]).toEqual([2, 2, PAGE_SIZE + 5]);
  });
  it("counts facets, most used first", () => {
    expect(facets(items).categories).toEqual([["game", 2], ["vpn", 2]]);
    expect(facets(items).tags[0]).toEqual(["tunnel", 2]);
  });
});

describe("filter in the URL", () => {
  it("round-trips and leaves other params alone", () => {
    const f = { ...base, q: "mc", categories: ["game"], tags: ["a-b", "c-d"], signedOnly: true, sort: "updated" as const, page: 3 };
    const params = filterToParams(f, new URLSearchParams("keep=1"));
    expect(params.get("keep")).toBe("1");
    expect(filterFromParams(params, false)).toEqual(f);
  });
  it("omits defaults so the plain page has a plain URL", () => {
    expect(filterToParams(base, new URLSearchParams()).toString()).toBe("");
  });
  it("ignores garbage and a popular sort nobody can offer", () => {
    const f = filterFromParams(new URLSearchParams("sort=popular&page=-4&signed=maybe"), false);
    expect([f.sort, f.page, f.signedOnly]).toEqual(["name", 1, false]);
    expect(filterFromParams(new URLSearchParams("sort=popular"), true).sort).toBe("popular");
  });
});
