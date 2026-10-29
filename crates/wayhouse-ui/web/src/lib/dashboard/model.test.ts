import { describe, expect, it } from "vitest";
import { dashboardNameProblem, exportDashboard, move, parseDashboard } from "./model";

describe("parseDashboard", () => {
  it("round-trips an export and keeps an unknown widget id", () => {
    const d = { name: "Ops", widgets: [{ id: "nodes.v1", width: 4 }, { id: "future.v9", width: 12, settings: { top: 3 } }] };
    expect(parseDashboard(exportDashboard(d))).toEqual(d);
  });
  it("names what is wrong", () => {
    expect(() => parseDashboard("{")).toThrow(/Not valid JSON/);
    expect(() => parseDashboard("[]")).toThrow(/JSON object/);
    expect(() => parseDashboard('{"widgets":{}}')).toThrow(/widgets/);
    expect(() => parseDashboard('{"widgets":[{"width":4}]}')).toThrow(/widgets\[0\]\.id/);
    expect(() => parseDashboard('{"widgets":[{"id":"a","width":13}]}')).toThrow(/widgets\[0\]\.width must be an integer from 1 to 12/);
  });
});

describe("dashboardNameProblem", () => {
  it("follows the controller's rule", () => {
    expect(dashboardNameProblem("Night shift")).toBeNull();
    expect(dashboardNameProblem("")).toMatch(/name/);
    expect(dashboardNameProblem("a/b")).toMatch(/cannot contain/);
    expect(dashboardNameProblem("Ops", ["Ops"])).toMatch(/already exists/);
  });
});

describe("move", () => {
  it("moves by one and stops at the ends", () => {
    expect(move([1, 2, 3], 1, -1)).toEqual([2, 1, 3]);
    expect(move([1, 2, 3], 1, 1)).toEqual([1, 3, 2]);
    const same = [1, 2, 3];
    expect(move(same, 0, -1)).toBe(same);
  });
});
