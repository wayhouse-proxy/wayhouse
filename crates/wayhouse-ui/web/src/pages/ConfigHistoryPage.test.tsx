import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  listRevisions: vi.fn(),
  diffRevision: vi.fn(),
  rollbackTo: vi.fn(),
  promoteRevision: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { ConfigHistoryPage } from "./ConfigHistoryPage";
import { SessionProvider } from "../lib/session";

beforeEach(() => {
  api.listRevisions.mockReset().mockResolvedValue([
    { revision: 2, size_bytes: 10, current: true },
    { revision: 1, size_bytes: 8, current: false },
  ]);
  api.rollbackTo.mockReset().mockResolvedValue({ revision: 3 });
});

describe("ConfigHistoryPage canary", () => {
  it("marks a revision visible to one group only and promotes it after asking", async () => {
    api.listRevisions.mockResolvedValue([
      { revision: 3, size_bytes: 10, current: true, promoted: false, canary_groups: ["eu"] },
      { revision: 2, size_bytes: 9, current: false, promoted: true, canary_groups: [] },
    ]);
    api.promoteRevision.mockResolvedValue(undefined);
    render(<ConfigHistoryPage />);
    expect(await screen.findByText("canary: eu")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "promote to everyone" })).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: "promote to everyone" }));
    expect(api.promoteRevision).not.toHaveBeenCalled();
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Promote" }));
    expect(api.promoteRevision).toHaveBeenCalledWith(3);
    expect(await screen.findByText(/promoted revision 3 to everyone/)).toBeInTheDocument();
  });
});

describe("ConfigHistoryPage rollback", () => {
  it("asks first, naming the revision, and only then rolls back", async () => {
    render(<ConfigHistoryPage />);
    await userEvent.click(await screen.findByRole("button", { name: "roll back to this" }));
    expect(api.rollbackTo).not.toHaveBeenCalled();
    const dlg = screen.getByRole("dialog");
    expect(dlg).toHaveTextContent("revision 1");
    await userEvent.click(within(dlg).getByRole("button", { name: "Roll back" }));
    expect(api.rollbackTo).toHaveBeenCalledWith(1);
    expect(await screen.findByText(/as new revision 3/)).toBeInTheDocument();
  });

  it("cancelling leaves the config untouched", async () => {
    render(<ConfigHistoryPage />);
    await userEvent.click(await screen.findByRole("button", { name: "roll back to this" }));
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.rollbackTo).not.toHaveBeenCalled();
  });

  it("disables rollback for a role below admin", async () => {
    render(
      <SessionProvider value={{ username: "ana", role: "operator" }}>
        <ConfigHistoryPage />
      </SessionProvider>,
    );
    expect(await screen.findByRole("button", { name: "roll back to this" })).toHaveAttribute("aria-disabled", "true");
  });
});
