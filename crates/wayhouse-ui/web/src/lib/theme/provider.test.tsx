import { afterEach, describe, expect, it } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { applyTheme, ThemeProvider, useTheme } from "./provider";
import { BUILTIN, type Theme } from "./builtin";

const night: Theme = { name: "Night", base: "dark", tokens: { select: "#7fb0ff" } };

afterEach(() => {
  applyTheme(document.documentElement, "system");
  localStorage.clear();
});

describe("applyTheme", () => {
  it("system leaves data-theme off so the OS decides", () => {
    applyTheme(document.documentElement, "dark");
    applyTheme(document.documentElement, "system");
    expect(document.documentElement.dataset.theme).toBeUndefined();
  });

  it("a custom theme sets every token, the base's values under the overrides", () => {
    const root = document.documentElement;
    applyTheme(root, "custom:Night", [night]);
    expect(root.dataset.theme).toBe("custom");
    expect(root.style.getPropertyValue("--select")).toBe("#7fb0ff");
    expect(root.style.getPropertyValue("--bg")).toBe(BUILTIN.dark.bg);
    expect(root.style.colorScheme).toBe("dark");
  });

  it("a deleted custom theme falls back to the system setting and clears its variables", () => {
    const root = document.documentElement;
    applyTheme(root, "custom:Night", [night]);
    applyTheme(root, "custom:Night", []);
    expect(root.dataset.theme).toBeUndefined();
    expect(root.style.getPropertyValue("--select")).toBe("");
  });
});

function Probe() {
  const { choice, setChoice, preview } = useTheme();
  return (
    <>
      <span>choice:{choice}</span>
      <button onClick={() => setChoice("dark")}>dark</button>
      <button onClick={() => preview(night)}>preview</button>
      <button onClick={() => preview(null)}>stop</button>
    </>
  );
}

describe("ThemeProvider", () => {
  it("remembers the choice per browser and applies it", async () => {
    render(<ThemeProvider><Probe /></ThemeProvider>);
    await userEvent.click(screen.getByText("dark"));
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(localStorage.getItem("wayhouse.theme")).toBe("dark");
  });

  it("a preview applies without being remembered", async () => {
    render(<ThemeProvider><Probe /></ThemeProvider>);
    await userEvent.click(screen.getByText("preview"));
    expect(document.documentElement.style.getPropertyValue("--select")).toBe("#7fb0ff");
    expect(localStorage.getItem("wayhouse.theme")).toBeNull();
    await userEvent.click(screen.getByText("stop"));
    expect(document.documentElement.style.getPropertyValue("--select")).toBe("");
  });

  it("works when storage throws", async () => {
    const get = Storage.prototype.getItem;
    Storage.prototype.getItem = () => { throw new Error("blocked"); };
    try {
      render(<ThemeProvider><Probe /></ThemeProvider>);
      expect(screen.getByText("choice:system")).toBeInTheDocument();
      await act(async () => {});
    } finally {
      Storage.prototype.getItem = get;
    }
  });
});
