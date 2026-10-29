import { describe, expect, it } from "vitest";
import { optionsForm } from "./options";

describe("optionsForm", () => {
  it("has no fields without a manifest", () => {
    expect(optionsForm(null).hasFields).toBe(false);
  });

  it("keeps secret options out of the form and takes group and order into the layout", () => {
    const f = optionsForm({
      name: "x",
      bindings: [],
      options: {
        type: "object",
        required: ["panel", "token"],
        properties: {
          panel: { type: "string", title: "Panel", group: "Connection", order: 1 },
          token: { type: "string", "x-wayhouse-secret": true },
          label: { type: "string" },
        },
      },
    });
    expect(Object.keys(f.schema.properties)).toEqual(["panel", "label"]);
    expect(f.schema.required).toEqual(["panel"]);
    expect(f.secrets).toEqual(["token"]);
    expect(f.layout.fields).toEqual({ panel: { group: "Connection", order: 1 } });
    expect(f.hasFields).toBe(true);
  });
});
