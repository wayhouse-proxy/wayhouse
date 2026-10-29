import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { DEFAULT_LAYOUT, knownPaths, mergeLayout, parseOverride, patternOf } from "./layout";
import { coerce, simplify, subschema, unitFor } from "./schema";

const SCHEMA = simplify(JSON.parse(readFileSync(`${import.meta.dirname}/../../../../../wayhouse-config/schema/config.schema.json`, "utf8")));
const KNOWN = knownPaths(SCHEMA);

describe("layout", () => {
  it("every path of the shipped default layout exists in the schema", () => {
    expect(Object.keys(DEFAULT_LAYOUT.fields).filter((p) => !KNOWN.has(p))).toEqual([]);
  });

  it("looks up list items with *", () => {
    expect(patternOf(["listeners", 0, "bind"])).toBe("listeners.*.bind");
    expect(KNOWN.has("listeners.*.bind")).toBe(true);
  });

  it("an override ignores unknown paths and keys with a warning, never an error", () => {
    const r = parseOverride(JSON.stringify({ version: 1, fields: { "/settings/workers": { help: "Tasks", color: "red", label: "Threads" }, "/settings/nope": { help: "x" } } }), KNOWN);
    expect(r.error).toBeUndefined();
    expect(r.layout.fields).toEqual({ "settings.workers": { help: "Tasks" } });
    expect(r.warnings).toHaveLength(3);
  });

  it.each([["{", /not valid JSON/], ['{"version":2,"fields":{}}', /must be an object/], ['{"version":1}', /must be an object/]])(
    "rejects %s as a whole",
    (text, msg) => expect(parseOverride(text, KNOWN).error).toMatch(msg),
  );

  it("merges field by field over the default", () => {
    const m = mergeLayout(DEFAULT_LAYOUT, { version: 1, fields: { "settings.workers": { label: "Threads" } } });
    expect(m.fields["settings.workers"]).toMatchObject({ label: "Threads", group: "General" });
    expect(mergeLayout(DEFAULT_LAYOUT, null)).toBe(DEFAULT_LAYOUT);
  });
});

describe("schema helpers", () => {
  it("turns 'or null' into an optional field and string constants into one enum", () => {
    const s = simplify({ properties: { a: { anyOf: [{ $ref: "#/$defs/X" }, { type: "null" }], description: "d" }, b: { type: ["integer", "null"], default: null } }, $defs: { B: { oneOf: [{ const: "x", type: "string" }, { enum: ["y", "z"], type: "string" }] } } });
    expect(s.properties.a).toEqual({ $ref: "#/$defs/X", description: "d" });
    expect(s.properties.b).toEqual({ type: "integer" });
    expect(s.$defs.B.enum).toEqual(["x", "y", "z"]);
  });

  it("keeps since annotations through the simplification", () => {
    expect(simplify(JSON.parse(readFileSync(`${import.meta.dirname}/../../../../../wayhouse-config/schema/config.schema.json`, "utf8"))).properties.web["x-wayhouse-since"]).toBe(3);
  });

  it("an integer-or-string port keeps digits as a number and a range as text", () => {
    const port = Object.values<any>(SCHEMA.$defs).find((d) => d["x-wayhouse-int-or-string"]);
    expect(port).toBeTruthy();
    const root = { $defs: { P: port }, properties: { p: { $ref: "#/$defs/P" } }, type: "object" };
    expect(coerce(root, root, { p: "25565" })).toEqual({ p: 25565 });
    expect(coerce(root, root, { p: "25565-25570" })).toEqual({ p: "25565-25570" });
  });

  it("units come from the schema, else the field name", () => {
    expect(unitFor("connect_timeout_ms", {})).toBe("milliseconds");
    expect(unitFor("max_new_sessions_per_sec", {})).toBe("per second");
    expect(unitFor("workers", {})).toBeUndefined();
    expect(unitFor("workers", { "x-wayhouse-unit": "tasks" })).toBe("tasks");
  });

  it("finds a sub-schema by path", () => {
    expect(subschema(SCHEMA, ["settings", "admin"])?.properties.listen).toBeTruthy();
    expect(subschema(SCHEMA, ["listeners", 0])?.properties.bind).toBeTruthy();
    expect(subschema(SCHEMA, ["nope"])).toBeUndefined();
  });
});

describe("useLayout", () => {
  it("falls back to the default with a notice when the override is unusable", async () => {
    const { renderHook } = await import("@testing-library/react");
    const { useLayout } = await import("./useLayout");
    const raw = JSON.parse(readFileSync(`${import.meta.dirname}/../../../../../wayhouse-config/schema/config.schema.json`, "utf8"));
    expect(renderHook(() => useLayout(raw, null)).result.current).toEqual({ layout: DEFAULT_LAYOUT, notice: null });
    const bad = renderHook(() => useLayout(raw, "{nope")).result.current;
    expect(bad.layout).toBe(DEFAULT_LAYOUT);
    expect(bad.notice).toMatch(/not used/);
    const warn = renderHook(() => useLayout(raw, '{"version":1,"fields":{"/settings/workers":{"help":"T"},"/x/y":{}}}')).result.current;
    expect(warn.layout.fields["settings.workers"].help).toBe("T");
    expect(warn.notice).toMatch(/\/x\/y/);
  });
});
