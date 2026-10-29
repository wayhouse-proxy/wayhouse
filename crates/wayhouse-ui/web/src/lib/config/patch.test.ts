import { describe, expect, it } from "vitest";
import { diff, parseText, patchText } from "./patch";

const FILE = `# fleet edge, eu
settings:
  workers: 2   # one per core
  admin:
    listen: "127.0.0.1:9901"   # loopback only
  mystery_key: keep-me

pools:
  # the default pool
  - name: eu
    targets: ["10.0.0.1:25565"]
listeners:
  - name: mc
    bind: "0.0.0.0:25565"
    pool: eu
`;

const edit = (fn: (v: any) => void, text = FILE) => {
  const before = parseText(text).value as any;
  const after = structuredClone(before);
  fn(after);
  return patchText(text, before, after);
};

describe("patchText", () => {
  it("round trips a form edit with comments, key order and unknown keys intact (RF-3)", () => {
    const out = edit((v) => (v.settings.workers = 4));
    expect(out).toBe(FILE.replace("workers: 2   #", "workers: 4 #").replace('"127.0.0.1:9901"   #', '"127.0.0.1:9901" #'));
  });

  it("an edit that changes nothing returns the text byte for byte", () => {
    expect(edit(() => {})).toBe(FILE);
  });

  it("keeps unknown keys the form never saw", () => {
    expect(parseText(edit((v) => (v.pools[0].name = "us"))).value).toMatchObject({ settings: { mystery_key: "keep-me" } });
  });

  it("adds a key at the end of its map and creates a missing parent", () => {
    const out = edit((v) => {
      v.settings.gossip = { bind: "0.0.0.0:7946" };
      v.settings.geo_db = "/db.mmdb";
    });
    expect(parseText(out).value).toMatchObject({ settings: { gossip: { bind: "0.0.0.0:7946" }, geo_db: "/db.mmdb" } });
    expect(out).toContain("# loopback only");
  });

  it("deletes a cleared field and keeps the rest", () => {
    const out = edit((v) => (v.settings.workers = undefined));
    expect(out).not.toContain("workers");
    expect(out).toContain("# loopback only");
  });

  it("appends and removes list items without touching their neighbours", () => {
    const added = edit((v) => v.listeners.push({ name: "udp", bind: "0.0.0.0:19132", protocol: "udp" }));
    expect(added).toContain("# the default pool");
    expect(parseText(added).value).toMatchObject({ listeners: [{ name: "mc" }, { name: "udp", protocol: "udp" }] });
    const removed = edit((v) => v.listeners.pop());
    expect(parseText(removed).value).toMatchObject({ listeners: [] });
  });

  it("starts a document from empty text", () => {
    expect(parseText(patchText("", {}, { settings: { workers: 1 } })).value).toEqual({ settings: { workers: 1 } });
  });

  it("refuses text that does not parse instead of overwriting it", () => {
    expect(() => patchText("a: [1,", {}, { a: 2 })).toThrow();
  });
});

describe("diff", () => {
  it("is empty for equal values and names only the changed path", () => {
    expect(diff({ a: { b: 1, c: 2 } }, { a: { b: 1, c: 2 } })).toEqual([]);
    expect(diff({ a: { b: 1, c: 2 } }, { a: { b: 1, c: 3 } })).toEqual([{ kind: "set", path: ["a", "c"], value: 3 }]);
  });
});
