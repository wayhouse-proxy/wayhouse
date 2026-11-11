import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SessionProvider, type Role } from "../lib/session";
import type { ProcessList } from "../types";

const api = vi.hoisted(() => ({
  listProcess: vi.fn(),
  applyProcess: vi.fn(),
  confirmProcess: vi.fn(),
  undoProcess: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { ProcessFlagsPage } from "./ProcessFlagsPage";

const flags = [
  { name: "listen", help: "Address to listen on.", kind: "text", value_name: "ADDR", default: "127.0.0.1:9901", editable: true, reason: null },
  { name: "plugins", help: "Enable plugins.", kind: "bool", value_name: null, default: null, editable: true, reason: null },
  { name: "ha-peers", help: "Peers.", kind: "list", value_name: "ADDR", default: null, editable: true, reason: null },
  { name: "auth-token", help: "Token.", kind: "text", value_name: null, default: null, editable: false, reason: "a credential" },
] as const;

const list = (over: Partial<ProcessList["programs"][number]> = {}): ProcessList => ({
  dir_configured: true,
  programs: [
    { program: "controller", label: "Controller", connected: true, flags: [...flags], file: { rev: 4, values: {}, previous: null, pending: null, error: null }, ...over },
    { program: "aggregator", label: "Aggregator", connected: false, flags: [], file: null },
  ],
});

const page = (role: Role = "admin") =>
  render(
    <SessionProvider value={{ username: "ana", role }}>
      <ProcessFlagsPage />
    </SessionProvider>,
  );

beforeEach(() => {
  Object.values(api).forEach((f) => f.mockReset());
  api.listProcess.mockResolvedValue(list());
  api.applyProcess.mockResolvedValue({ rev: 5 });
});

describe("ProcessFlagsPage", () => {
  it("explains a UI without a settings directory", async () => {
    api.listProcess.mockResolvedValue({ dir_configured: false, programs: [] });
    page();
    expect(await screen.findByText(/started without/i)).toBeInTheDocument();
  });

  it("shows editable flags, names the locked ones and marks a program that never connected", async () => {
    page();
    expect(await screen.findByLabelText("--listen")).toBeInTheDocument();
    expect(screen.queryByLabelText("--auth-token")).toBeNull();
    expect(screen.getByText(/Not editable here: --auth-token \(a credential\)/)).toBeInTheDocument();
    const closed = screen.getAllByRole("button").find((b) => b.getAttribute("aria-expanded") === "false");
    expect(closed).toBeDefined();
    await userEvent.click(closed!);
    expect(screen.getByText(/Not connected\./)).toBeInTheDocument();
  });

  it("applies only what changed, after a confirmation that says it restarts", async () => {
    const user = userEvent.setup();
    page();
    await user.type(await screen.findByLabelText("--listen"), "0.0.0.0:9901");
    await user.click(screen.getByRole("switch", { name: "--plugins" }));
    await user.click(screen.getByRole("button", { name: "Apply as a trial" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/restarts now/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Restart with the change" }));
    expect(api.applyProcess).toHaveBeenCalledWith("controller", { listen: "0.0.0.0:9901", plugins: true }, 4);
  });

  it("keeps new lines while typing a list and sends the entries", async () => {
    const user = userEvent.setup();
    page();
    const peers = await screen.findByLabelText("--ha-peers");
    await user.type(peers, "10.0.0.1:7000{Enter}10.0.0.2:7000");
    expect(peers).toHaveValue("10.0.0.1:7000\n10.0.0.2:7000");
    await user.click(screen.getByRole("button", { name: "Apply as a trial" }));
    await user.click(await screen.findByRole("button", { name: "Restart with the change" }));
    expect(api.applyProcess).toHaveBeenCalledWith("controller", { "ha-peers": ["10.0.0.1:7000", "10.0.0.2:7000"] }, 4);
  });

  it("keeps text typed in one program while another program's trial keeps the page polling", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      // A fresh object graph on every call, like a real reload.
      api.listProcess.mockImplementation(async () => ({
        dir_configured: true,
        programs: [
          {
            program: "controller", label: "Controller", connected: true, flags: [...flags],
            file: { rev: 5, values: {}, previous: {}, pending: { applied_at: 1, attempts: 1, confirm_secs: 60 }, error: null },
          },
          {
            program: "aggregator", label: "Aggregator", connected: true, flags: [{ ...flags[0] }],
            file: { rev: 2, values: {}, previous: null, pending: null, error: null },
          },
        ],
      }));
      page();
      const box = await screen.findByLabelText("--listen", { selector: "input:not([disabled])" });
      await user.type(box, "0.0.0.0:1");
      const calls = api.listProcess.mock.calls.length;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3500);
      });
      expect(api.listProcess.mock.calls.length).toBeGreaterThan(calls);
      expect(box).toHaveValue("0.0.0.0:1");
    } finally {
      vi.useRealTimers();
    }
  });

  it("offers Confirm and Undo while a change is on trial, and blocks new edits", async () => {
    const user = userEvent.setup();
    api.listProcess.mockResolvedValue(
      list({ file: { rev: 5, values: { listen: "0.0.0.0:9" }, previous: {}, pending: { applied_at: 1, attempts: 1, confirm_secs: 60 }, error: null } }),
    );
    api.confirmProcess.mockResolvedValue({ rev: 6 });
    page();
    expect(await screen.findByText(/on trial/)).toBeInTheDocument();
    expect(screen.getByLabelText("--listen")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Apply as a trial" })).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("button", { name: "Confirm" }));
    expect(api.confirmProcess).toHaveBeenCalledWith("controller", 5);
  });

  it("shows why the program refused a change and went back", async () => {
    api.listProcess.mockResolvedValue(
      list({ file: { rev: 6, values: {}, previous: null, pending: null, error: "invalid value for --listen" } }),
    );
    page();
    expect(await screen.findByText(/refused the last change and went back: invalid value for --listen/)).toBeInTheDocument();
  });

  it("shows a refusal from the server in place", async () => {
    const user = userEvent.setup();
    const { ApiError } = await vi.importActual<typeof import("../api")>("../api");
    api.applyProcess.mockRejectedValue(new ApiError(409, "the settings changed since you loaded them; reload and try again"));
    page();
    await user.type(await screen.findByLabelText("--listen"), "x");
    await user.click(screen.getByRole("button", { name: "Apply as a trial" }));
    await user.click(await screen.findByRole("button", { name: "Restart with the change" }));
    expect(await screen.findByText(/changed since you loaded them/)).toBeInTheDocument();
  });

  it("is read-only for a viewer", async () => {
    page("viewer");
    expect(await screen.findByLabelText("--listen")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Apply as a trial" })).toHaveAttribute("aria-disabled", "true");
  });
});
