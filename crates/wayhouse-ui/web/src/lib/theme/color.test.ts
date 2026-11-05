import { describe, expect, it } from "vitest";
import { deltaE00Lab, hsl, parseHex } from "./color";

describe("deltaE00Lab", () => {
  // Published test data: Sharma, Wu and Dalal 2005, Table 1 (pairs 1, 2, 7, 9, 13, 18 and 34). The Rust port must match these.
  it.each([
    [[50, 2.6772, -79.7751], [50, 0, -82.7485], 2.0425],
    [[50, 3.1571, -77.2803], [50, 0, -82.7485], 2.8615],
    [[50, 2.5, 0], [50, 0, -2.5], 4.3065],
    [[50, 2.5, 0], [73, 25, -18], 27.1492],
    [[50, 2.5, 0], [61, -5, 29], 22.8977],
    [[60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644],
    [[2.0776, 0.0795, -1.135], [0.9033, -0.0636, -0.5514], 0.9082],
  ] as const)("%j vs %j is %f", (a, b, want) => {
    expect(deltaE00Lab([...a], [...b])).toBeCloseTo(want, 3);
  });
});

describe("hsl", () => {
  it("gives hue in degrees, saturation and lightness in 0..1", () => {
    const near = (hex: string, want: [number, number, number]) => {
      const got = hsl(parseHex(hex)!);
      want.forEach((w, i) => expect(got[i]).toBeCloseTo(w, 2));
    };
    near("#ff0000", [0, 1, 0.5]);
    near("#00ff00", [120, 1, 0.5]);
    near("#0000ff", [240, 1, 0.5]);
    near("#808080", [0, 0, 0.5]);
    near("#ffbf00", [44.94, 1, 0.5]);
  });
});
