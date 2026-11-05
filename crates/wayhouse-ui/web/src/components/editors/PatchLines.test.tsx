import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PatchLines } from "./PatchLines";

describe("PatchLines", () => {
  it("draws no 'up' mark for the listener itself: nothing here measures that it is listening", () => {
    const { container } = render(<PatchLines listener={{ name: "voice", routes: [{ match: { type: "always" }, action: { pool: "eu" } }] }} stateOf={() => "down"} />);
    expect(container.querySelector("circle.fill-good")).toBeNull();
  });
});
