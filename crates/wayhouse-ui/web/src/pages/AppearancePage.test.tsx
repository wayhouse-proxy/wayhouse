import { render, screen, waitFor } from "@testing-library/react";
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
});
