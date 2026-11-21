import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PoolLines } from "./PoolLines";

describe("PoolLines", () => {
  it("clips a long backend address in the drawing and keeps the whole one as its tooltip", () => {
    const addr = "game-backend-with-a-very-long-name.example.internal:25565";
    const { container } = render(<PoolLines name="main" rows={[{ addr, state: "up" }]} />);
    const text = container.querySelector("svg g text")!;
    expect(text.textContent).toContain("…");
    expect(text.textContent!.length).toBeLessThan(addr.length);
    expect(container.querySelector("svg title")?.textContent).toBe(addr);
    // the table underneath still has the full address
    expect(container.querySelector("td")?.textContent).toBe(addr);
  });
  it("leaves a short address alone", () => {
    const { container } = render(<PoolLines name="main" rows={[{ addr: "10.0.0.1:25565", state: "up" }]} />);
    expect(container.querySelector("svg g text")!.textContent).toBe("10.0.0.1:25565");
  });
});
