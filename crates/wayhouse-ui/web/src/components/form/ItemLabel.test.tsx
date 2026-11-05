import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { SchemaForm } from "./SchemaForm";

const SCHEMA = {
  type: "object",
  properties: { node_ids: { type: "array", title: "Node IDs", items: { type: "integer" }, default: [1, 2] } },
};

describe("a list of numbers", () => {
  it("labels each entry with the list's name and its position, never a generated suffix", () => {
    render(<SchemaForm schema={SCHEMA} value={{ node_ids: [1, 2] }} onChange={() => {}} />);
    const labels = screen.getAllByRole("spinbutton").map((e) => e.getAttribute("aria-label") ?? document.querySelector(`label[for="${e.id}"]`)?.textContent);
    expect(labels).toEqual(["Node ID 1", "Node ID 2"]);
  });
});
