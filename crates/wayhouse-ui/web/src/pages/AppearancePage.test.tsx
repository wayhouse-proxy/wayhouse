import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  listThemes: vi.fn(),
  getUiDefaults: vi.fn(),
  saveTheme: vi.fn(),
  deleteTheme: vi.fn(),
  saveUiDefaults: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { QueryProvider } from "../lib/query";
import { SessionProvider, type Role } from "../lib/session";
import { ThemeProvider } from "../lib/theme/provider";
import { AppearancePage } from "./AppearancePage";

const night = { name: "Night", base: "dark", tokens: { select: "#7fb0ff" } };

function show(role: Role) {
  return render(
    <SessionProvider value={{ username: "ana", role }}>
      <QueryProvider>
        <ThemeProvider>
          <AppearancePage />
        </ThemeProvider>
      </QueryProvider>
    </SessionProvider>,
  );
}

beforeEach(() => {
  localStorage.clear();
  api.listThemes.mockReset().mockResolvedValue([night]);
  api.getUiDefaults.mockReset().mockResolvedValue({});
  api.saveTheme.mockReset().mockResolvedValue(night);
  api.deleteTheme.mockReset().mockResolvedValue(undefined);
  api.saveUiDefaults.mockReset().mockResolvedValue({});
});

describe("AppearancePage", () => {
  it("lists the stored themes and remembers this browser's pick locally", async () => {
    show("viewer");
    expect(await screen.findByRole("cell", { name: "Night" })).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText("This browser"), "dark");
    expect(localStorage.getItem("wayhouse.theme")).toBe("dark");
  });

  it("a viewer cannot change the deployment default", async () => {
    show("viewer");
    await screen.findByRole("cell", { name: "Night" });
    expect(screen.getByLabelText("Deployment default")).toBeDisabled();
  });

  it("an admin saves the deployment default", async () => {
    show("admin");
    await screen.findByRole("cell", { name: "Night" });
    await userEvent.selectOptions(screen.getByLabelText("Deployment default"), "Night");
    await waitFor(() => expect(api.saveUiDefaults).toHaveBeenCalled());
    expect(api.saveUiDefaults.mock.calls[0][0]).toEqual({ theme: "Night" });
  });

  it("renaming the default theme moves the default before the old name is deleted", async () => {
    api.getUiDefaults.mockResolvedValue({ theme: "Night" });
    show("admin");
    await screen.findByRole("cell", { name: "Night" });
    await userEvent.click(screen.getByRole("button", { name: "Edit Night" }));
    const name = screen.getByLabelText(/^Name/);
    await userEvent.clear(name);
    await userEvent.type(name, "Night 2");
    await userEvent.click(screen.getByRole("button", { name: /^Save/ }));
    await waitFor(() => expect(api.deleteTheme).toHaveBeenCalled());
    expect(api.saveUiDefaults.mock.calls[0][0]).toEqual({ theme: "Night 2" });
    expect(api.saveUiDefaults.mock.invocationCallOrder[0]).toBeLessThan(api.deleteTheme.mock.invocationCallOrder[0]);
  });

  it("shows the controller's refusal when a theme cannot be saved", async () => {
    api.saveTheme.mockRejectedValue(new Error("themes is full (50)"));
    show("admin");
    await screen.findByRole("cell", { name: "Night" });
    await userEvent.click(screen.getByRole("button", { name: "Edit Night" }));
    await userEvent.click(screen.getByRole("button", { name: /^Save/ }));
    expect(await screen.findByText(/themes is full/)).toBeInTheDocument();
  });

  it("an imported theme with a failing contrast pair cannot be saved", async () => {
    show("admin");
    await screen.findByRole("cell", { name: "Night" });
    const bad = JSON.stringify({ name: "Flat", base: "light", tokens: { ink: "#ffffff", bg: "#ffffff" } });
    fireEvent.change(screen.getByLabelText("Import a theme file"), { target: { files: [new File([bad], "flat.json", { type: "application/json" })] } });
    expect(await screen.findByText(/checks? failing/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Save/ })).toHaveAttribute("aria-disabled", "true");
  });
});
