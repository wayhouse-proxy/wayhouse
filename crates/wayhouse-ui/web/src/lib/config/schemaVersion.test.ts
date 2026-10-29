import { describe, expect, it } from "vitest";
import { requiredSchemaVersion } from "./schemaVersion";

const schema = {
  type: "object",
  properties: {
    schema_version: { type: "integer" },
    settings: {
      type: "object",
      properties: {
        workers: { type: "integer" },
        features: { type: "object", "x-wayhouse-since": 4, properties: { x: { type: "boolean" } } },
      },
    },
    web: { anyOf: [{ type: "null" }, { $ref: "#/$defs/Web" }], "x-wayhouse-since": 3 },
  },
  $defs: { Web: { type: "object", properties: { sites: { type: "array", items: { type: "string" } } } } },
};

describe("requiredSchemaVersion", () => {
  it("is 1 when nothing newer is touched", () => {
    expect(requiredSchemaVersion(schema, { settings: { workers: 1 } }, { settings: { workers: 2 } })).toBe(1);
  });

  it("is the highest 'since' on the path of a set value, at any depth", () => {
    expect(requiredSchemaVersion(schema, {}, { settings: { features: { x: true } } })).toBe(4);
    expect(requiredSchemaVersion(schema, {}, { web: { sites: ["a"] } })).toBe(3);
  });

  it("ignores deletions: removing a field never needs a newer schema", () => {
    expect(requiredSchemaVersion(schema, { settings: { features: { x: true } } }, { settings: {} })).toBe(1);
  });
});
