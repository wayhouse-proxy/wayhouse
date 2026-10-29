import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  getFleetStats: vi.fn(),
  listRevisions: vi.fn(),
  listAcmeSites: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));
const store = vi.hoisted(() => ({ list: vi.fn(), save: vi.fn(), del: vi.fn() }));
vi.mock("../lib/dashboard/model", async (orig) => ({
  ...(await orig<typeof import("../lib/dashboard/model")>()),
  listDashboards: store.list,
  saveDashboard: store.save,
  deleteDashboard: store.del,
}));
vi.mock("../useFleetSocket", () => ({
  useFleetSocket: () => ({
    connected: true,
    instances: [
      { instance: "eu-1", last_seen_ms_ago: 1000, stale: false, pools: [{ name: "web", balancer: "rr", backends: [{ addr: "10.0.0.1:80", healthy: false, state: "enabled", active: 0 }] }], sessions: { tcp: 1, udp: 0 }, version: "0.4.0", skew: "none" },
      { instance: "eu-2", last_seen_ms_ago: 90000, stale: true, pools: [], sessions: { tcp: 0, udp: 0 }, version: "0.3.0", skew: "outside-window" },
    ],
  }),
}));

import { QueryProvider } from "../lib/query";
import { SessionProvider, type Role } from "../lib/session";
import { ToastProvider } from "../components/ui/Toast";
import { InspectorPanel, InspectorProvider } from "../components/shell/Inspector";
import { issuesOf } from "../components/dashboard/widgets";
import { DashboardPage } from "./DashboardPage";

function show(role: Role) {
  return render(
    <SessionProvider value={{ username: "ana", role }}>
      <QueryProvider>
        <ToastProvider>
          <InspectorProvider>
            <MemoryRouter>
              <DashboardPage />
              <InspectorPanel />
            </MemoryRouter>
          </InspectorProvider>
        </ToastProvider>
      </QueryProvider>
    </SessionProvider>,
  );
}

const board = { name: "Ops", widgets: [{ id: "nodes.v1", width: 6 }, { id: "attention.v1", width: 6 }, { id: "future.v9", width: 4 }] };

beforeEach(() => {
  store.list.mockReset().mockResolvedValue([board]);
  store.save.mockReset().mockImplementation(async (d) => d);
  store.del.mockReset().mockResolvedValue(undefined);
  api.getFleetStats.mockReset().mockRejectedValue(new Error("aggregator not configured"));
  api.listRevisions.mockReset().mockResolvedValue([{ revision: 12, size_bytes: 1, current: true }]);
  api.listAcmeSites.mockReset().mockResolvedValue({ sites: [] });
});

describe("DashboardPage", () => {
  it("shows the stored widgets, a placeholder for an unknown id and the fleet's issues", async () => {
    show("operator");
    expect(await screen.findByRole("region", { name: "Nodes" })).toBeInTheDocument();
    expect(screen.getByText("This widget is not available")).toBeInTheDocument();
    const attention = screen.getByRole("region", { name: "Needs attention" });
    expect(within(attention).getByText(/eu-2 has not reported/)).toBeInTheDocument();
    expect(within(attention).getByText(/Showing 3 of 3/)).toBeInTheDocument();
  });

  it("one failing widget does not take the others down", async () => {
    api.getFleetStats.mockRejectedValue(new Error("aggregator not configured"));
    store.list.mockResolvedValue([{ name: "Ops", widgets: [{ id: "connections.v1", width: 6 }, { id: "nodes.v1", width: 6 }] }]);
    show("operator");
    expect(await screen.findByText(/aggregator not configured/, {}, { timeout: 4000 })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Nodes" })).toBeInTheDocument();
  });

  it("customize mode moves, resizes and removes with buttons, and saves on Done", async () => {
    show("operator");
    await screen.findByRole("region", { name: "Nodes" });
    await userEvent.click(screen.getByRole("button", { name: "Customize" }));
    await userEvent.click(screen.getByRole("button", { name: "Move Nodes later" }));
    await userEvent.selectOptions(screen.getByLabelText("Width of Nodes"), "12");
    await userEvent.click(screen.getByRole("button", { name: "Remove future.v9" }));
    await userEvent.click(screen.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(store.save).toHaveBeenCalled());
    expect(store.save.mock.calls[0][0]).toEqual({ name: "Ops", widgets: [{ id: "attention.v1", width: 6 }, { id: "nodes.v1", width: 12 }] });
    expect(await screen.findByText("Dashboard saved")).toBeInTheDocument();
  });

  it("keeps keyboard focus on the Move button and announces the move", async () => {
    show("operator");
    await screen.findByRole("region", { name: "Nodes" });
    await userEvent.click(screen.getByRole("button", { name: "Customize" }));
    const later = screen.getByRole("button", { name: "Move Nodes later" });
    await userEvent.click(later);
    expect(later).toHaveFocus();
    expect(screen.getAllByRole("status").some((el) => el.textContent === "Nodes moved to position 2 of 3")).toBe(true);
  });

  it("settings follow the widget, not its position", async () => {
    store.list.mockResolvedValue([{ name: "Ops", widgets: [{ id: "nodes.v1", width: 6 }, { id: "attention.v1", width: 6 }, { id: "throughput.v1", width: 6 }] }]);
    api.getFleetStats.mockResolvedValue({ generated_at_ms: 1, range_sec: 900, fleet: { resolution_sec: 60, last_sample_ms: null, points: [] }, listeners: [], pools: [], regions: [] });
    show("operator");
    await screen.findByRole("region", { name: "Nodes" });
    await userEvent.click(screen.getByRole("button", { name: "Customize" }));
    await userEvent.click(screen.getByRole("button", { name: "Settings of Throughput by region" }));
    await userEvent.click(screen.getByRole("button", { name: "Remove Nodes" }));
    const panel = screen.getByRole("complementary", { name: "Details" });
    expect(await within(panel).findByRole("heading", { name: "Throughput by region" })).toBeInTheDocument();
    expect(within(panel).getByLabelText("Time range")).toBeInTheDocument();
  });

  it("Reset goes back to the saved dashboard", async () => {
    show("operator");
    await screen.findByRole("region", { name: "Nodes" });
    await userEvent.click(screen.getByRole("button", { name: "Customize" }));
    await userEvent.click(screen.getByRole("button", { name: "Remove Nodes" }));
    expect(screen.queryByRole("region", { name: "Nodes" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Reset" }));
    expect(screen.getByRole("region", { name: "Nodes" })).toBeInTheDocument();
  });

  it("a viewer cannot customize", async () => {
    show("viewer");
    await screen.findByRole("region", { name: "Nodes" });
    expect(screen.getByRole("button", { name: "Customize" })).toHaveAttribute("aria-disabled", "true");
  });

  it("shows the default dashboard when none is stored", async () => {
    store.list.mockResolvedValue([]);
    show("operator");
    expect(await screen.findByText(/Showing the default dashboard/)).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Nodes" })).toBeInTheDocument();
  });
});

describe("issuesOf", () => {
  it("puts down before degraded", () => {
    const [first, ...rest] = issuesOf([
      { instance: "a", last_seen_ms_ago: 0, stale: false, pools: [], sessions: { tcp: 0, udp: 0 }, skew: "outside-window", version: "0.1" },
      { instance: "b", last_seen_ms_ago: 99000, stale: true, pools: [], sessions: { tcp: 0, udp: 0 } },
    ]);
    expect(first.state).toBe("down");
    expect(rest[0].state).toBe("degraded");
  });
});

describe("a widget that throws", () => {
  it("is contained in its own panel", async () => {
    api.listAcmeSites.mockResolvedValue([]);
    store.list.mockResolvedValue([{ name: "Ops", widgets: [{ id: "certificates.v1", width: 6 }, { id: "nodes.v1", width: 6 }] }]);
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    show("operator");
    expect(await screen.findByText(/This widget failed to draw/)).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Nodes" })).toBeInTheDocument();
    err.mockRestore();
  });
});
