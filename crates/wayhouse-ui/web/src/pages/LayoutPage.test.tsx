import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  getLayoutOverride: vi.fn(),
  saveLayoutOverride: vi.fn(),
  deleteLayoutOverride: vi.fn(),
  getConfigSchema: vi.fn(),
  getCurrentConfig: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { QueryProvider } from "../lib/query";
import { SessionProvider, type Role } from "../lib/session";
import { LayoutPage } from "./LayoutPage";

const schema = { type: "object", properties: { settings: { type: "object", properties: { workers: { type: "integer", minimum: 1 } } } } };

function show(role: Role) {
  return render(
    <SessionProvider value={{ username: "ana", role }}>
      <QueryProvider>
        <LayoutPage />
      </QueryProvider>
    </SessionProvider>,
  );
}

beforeEach(() => {
  api.getLayoutOverride.mockReset().mockResolvedValue(null);
  api.saveLayoutOverride.mockReset().mockResolvedValue(undefined);
  api.deleteLayoutOverride.mockReset().mockResolvedValue(undefined);
  api.getConfigSchema.mockReset().mockResolvedValue(schema);
  api.getCurrentConfig.mockReset().mockResolvedValue({ text: "settings:\n  workers: 2\n", revision: 1 });
});

describe("LayoutPage", () => {
  it("saves a valid override and refuses broken JSON", async () => {
    show("admin");
    const box = await screen.findByLabelText("Layout JSON");
    await userEvent.click(box);
    await userEvent.paste("{");
    expect(await screen.findByText(/Unexpected|Expected|end of/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save layout" })).toHaveAttribute("aria-disabled", "true");
    await userEvent.clear(box);
    await userEvent.click(box);
    await userEvent.paste('{"version":1,"fields":{"/settings/workers":{"order":1}}}');
    await userEvent.click(screen.getByRole("button", { name: "Save layout" }));
    await waitFor(() => expect(api.saveLayoutOverride).toHaveBeenCalled());
    expect(api.saveLayoutOverride.mock.calls[0][0]).toBe('{"version":1,"fields":{"/settings/workers":{"order":1}}}');
  });

  it("a viewer can read but not save", async () => {
    show("viewer");
    await screen.findByLabelText("Layout JSON");
    expect(screen.getByRole("button", { name: "Save layout" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(/Only admins/)).toBeInTheDocument();
  });
});
