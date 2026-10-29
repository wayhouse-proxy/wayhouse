import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FleetInstanceView } from "../types";
import type { FleetNode, NodePage, Rollup, SavedFilter } from "../lib/fleet/nodes";

const REGIONS = ["eu", "us", "ap"];
/** 500 nodes: every 25th degraded, every 100th stale, tags by index. */
const FLEET: FleetNode[] = Array.from({ length: 500 }, (_, i) => ({
  instance: `n${String(i).padStart(3, "0")}`,
  state: i % 100 === 99 ? "stale" : i % 25 === 0 ? "degraded" : "ok",
  last_seen_ms_ago: 1000 + i,
  region: REGIONS[i % 3],
  group: undefined,
  tags: { env: i % 2 ? "prod" : "stage", ...(i % 10 === 0 ? { gpu: "a100" } : {}) },
  version: i === 1 ? "0.2.4" : "0.3.0",
  protocol: "1.1",
  skew: i === 1 ? "within-window" : i === 2 ? "outside-window" : "none",
  sessions: { tcp: 2, udp: 1 },
  backends: 1,
  unhealthy_backends: i % 25 === 0 ? 1 : 0,
}));

const rollup = (ns: FleetNode[]): Rollup => ({
  nodes: ns.length,
  degraded: ns.filter((n) => n.state === "degraded").length,
  stale: ns.filter((n) => n.state === "stale").length,
  sessions: ns.length * 3,
});
function select(query: string) {
  const p = new URLSearchParams(query);
  const q = (p.get("q") ?? "").toLowerCase();
  return FLEET.filter(
    (n) =>
      (!q || n.instance.includes(q)) &&
      (!p.get("state") || n.state === p.get("state")) &&
      (!p.get("region") || n.region === p.get("region")) &&
      p.getAll("tag").every((t) => {
        const [k, v] = t.split("=");
        return k in n.tags && (v === undefined || n.tags[k] === v);
      }),
  );
}
function pageOf(query: string): NodePage {
  const p = new URLSearchParams(query);
  const size = Number(p.get("size") ?? 25);
  const page = Number(p.get("page") ?? 1);
  const all = select(query).sort((a, b) => (a.region ?? "").localeCompare(b.region ?? "") || a.instance.localeCompare(b.instance));
  return {
    items: all.slice((page - 1) * size, page * size),
    total: all.length,
    page,
    size,
    rollup: rollup(all),
    groups: REGIONS.slice().sort().map((r) => ({ region: r, ...rollup(all.filter((n) => n.region === r)) })),
  };
}

const api = vi.hoisted(() => ({
  getFleetNodes: vi.fn(),
  resolveFleetNodes: vi.fn(),
  listFilters: vi.fn(),
  saveFilter: vi.fn(),
  deleteFilter: vi.fn(),
  drainInstance: vi.fn(),
  undrainInstance: vi.fn(),
  deleteBackend: vi.fn(),
  patchBackend: vi.fn(),
  addBackend: vi.fn(),
  routeHint: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

const live = vi.hoisted(() => ({ instances: [] as FleetInstanceView[] }));
vi.mock("../useFleetSocket", () => ({ useFleetSocket: () => ({ instances: live.instances, connected: true }) }));

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { SessionProvider, type Role } from "../lib/session";
import { ToastProvider } from "../components/ui/Toast";
import { InspectorPanel, InspectorProvider } from "../components/shell/Inspector";
import { FleetPage } from "./FleetPage";

const Where = () => <output data-testid="where">{useLocation().search}</output>;
function show(role: Role = "admin", url = "/fleet") {
  return render(
    <SessionProvider value={{ username: "ana", role }}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <ToastProvider>
          <InspectorProvider>
            <MemoryRouter initialEntries={[url]}>
              <FleetPage />
              <InspectorPanel />
              <Where />
            </MemoryRouter>
          </InspectorProvider>
        </ToastProvider>
      </QueryClientProvider>
    </SessionProvider>,
  );
}
const lastQuery = () => new URLSearchParams(api.getFleetNodes.mock.calls.at(-1)![0] as string);
const where = () => new URLSearchParams(screen.getByTestId("where").textContent!);

beforeEach(() => {
  vi.resetAllMocks();
  live.instances = [];
  api.getFleetNodes.mockImplementation(async (q: string) => pageOf(q));
  api.resolveFleetNodes.mockImplementation(async (q: string) => {
    const ns = select(q);
    return { count: ns.length, instances: ns.map((n) => n.instance) };
  });
  api.listFilters.mockResolvedValue([] as SavedFilter[]);
  api.saveFilter.mockImplementation(async (f: SavedFilter) => f);
  api.deleteFilter.mockResolvedValue(undefined);
  for (const m of [api.drainInstance, api.undrainInstance, api.deleteBackend, api.patchBackend]) m.mockResolvedValue("ok");
});

describe("Fleet page with 500 nodes", () => {
  it("shows one page grouped by region, with the rollup and 'Showing' over the whole selection", async () => {
    show();
    expect(await screen.findByText("Showing 1 to 25 of 500")).toBeInTheDocument();
    expect(screen.getByText(/Fleet: 500 nodes · 20 degraded · 5 stale · 1500 sessions/)).toBeInTheDocument();
    // The page is region-ordered, so the first region on it carries its own rollup from the server.
    expect(screen.getByRole("button", { name: /^ap/ })).toHaveTextContent("166 nodes");
    expect(screen.getAllByRole("row").length).toBeLessThan(40);
    // Streaming numbers are not a live region; only the pager's "Showing" line is.
    expect(screen.getByText(/Fleet: 500 nodes/)).not.toHaveAttribute("role");
  });

  it("changing the sort goes back to page 1", async () => {
    show("admin", "/fleet?page=3");
    await screen.findByText("Showing 51 to 75 of 500");
    await userEvent.selectOptions(screen.getByLabelText("Sort by"), "sessions");
    expect(await screen.findByText("Showing 1 to 25 of 500")).toBeInTheDocument();
    expect(where().get("page")).toBeNull();
  });

  it("pages on the server and keeps the page in the URL", async () => {
    show();
    await screen.findByText("Showing 1 to 25 of 500");
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("Showing 26 to 50 of 500")).toBeInTheDocument();
    expect(lastQuery().get("page")).toBe("2");
    expect(where().get("page")).toBe("2");
    await userEvent.selectOptions(screen.getByLabelText("Per page"), "100");
    expect(await screen.findByText("Showing 1 to 100 of 500")).toBeInTheDocument();
    expect(where().get("size")).toBe("100");
    expect(where().get("page")).toBeNull();
  });

  it("opens from a URL and applies it", async () => {
    show("admin", "/fleet?state=degraded&tag=env%3Dstage&page=1&size=25");
    expect(await screen.findByText(/Matching: 10 nodes/)).toBeInTheDocument();
    expect(lastQuery().get("state")).toBe("degraded");
    expect(lastQuery().getAll("tag")).toEqual(["env=stage"]);
    expect(lastQuery().get("group_by")).toBeNull(); // added by the api layer, not the page
    expect(screen.getByRole("button", { name: "Remove tag filter env=stage" })).toBeInTheDocument();
  });

  it("filters by text after a pause, by state, region and tag, and goes back to page 1", async () => {
    show();
    await screen.findByText("Showing 1 to 25 of 500");
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText("Showing 26 to 50 of 500");
    await userEvent.type(screen.getByLabelText("Search"), "n01");
    await waitFor(() => expect(where().get("q")).toBe("n01"));
    expect(await screen.findByText("Showing 1 to 10 of 10")).toBeInTheDocument();
    expect(where().get("page")).toBeNull();

    await userEvent.clear(screen.getByLabelText("Search"));
    await waitFor(() => expect(where().get("q")).toBeNull());
    await userEvent.selectOptions(screen.getByLabelText("State"), "stale");
    expect(await screen.findByText("Showing 1 to 5 of 5")).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText("State"), "");
    await userEvent.type(screen.getByLabelText("Tag"), "gpu=a100{Enter}");
    expect(await screen.findByText("Showing 1 to 25 of 50")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Remove tag filter gpu=a100" }));
    expect(await screen.findByText("Showing 1 to 25 of 500")).toBeInTheDocument();
  });

  it("says when nothing matches and offers to clear the filter", async () => {
    show("admin", "/fleet?q=nothing-like-this");
    expect(await screen.findByText("No nodes match this filter.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(await screen.findByText("Showing 1 to 25 of 500")).toBeInTheDocument();
  });

  it("flags version skew next to the version", async () => {
    show("admin", "/fleet?q=n00");
    await screen.findByText("Showing 1 to 10 of 10");
    expect(screen.getByText("older, in window")).toBeInTheDocument();
    expect(screen.getByText("outside window")).toBeInTheDocument();
  });

  it("shows skeleton rows while the first page loads, and an error with Retry", async () => {
    api.getFleetNodes.mockRejectedValueOnce(new Error("aggregator unreachable"));
    show();
    expect(screen.getByRole("status", { name: "Loading" })).toBeInTheDocument();
    expect(await screen.findByText(/aggregator unreachable/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Showing 1 to 25 of 500")).toBeInTheDocument();
  });

  it("renders a 100-row page of the 500-node fleet quickly enough to notice a regression", async () => {
    const t0 = performance.now();
    show("admin", "/fleet?size=100");
    await screen.findByText("Showing 1 to 100 of 500");
    // Noted, not a gate: jsdom time varies with the machine.
    console.info(`fleet 100-row page: ${Math.round(performance.now() - t0)} ms`);
    expect(screen.getAllByRole("row").length).toBeGreaterThan(100);
  });
});

describe("Fleet page inspector", () => {
  const row = () => screen.findByRole("row", { name: /n001/ });
  beforeEach(() => {
    live.instances = [
      {
        instance: "n001",
        last_seen_ms_ago: 800,
        stale: false,
        sessions: { tcp: 2, udp: 1 },
        pools: [{ name: "lobby", balancer: "rr", backends: [{ addr: "10.0.0.5:7777", healthy: true, state: "enabled", active: 2 }] }],
      },
    ];
  });

  it("opens a node from its row, keeps it in the URL and closes on Escape", async () => {
    show("admin", "/fleet?q=n00");
    await userEvent.click(await row());
    const panel = await screen.findByRole("complementary", { name: "Details" });
    expect(within(panel).getByRole("heading", { name: "n001" })).toBeInTheDocument();
    expect(panel).toHaveTextContent("v0.2.4 · protocol 1.1");
    expect(panel).toHaveTextContent("env=prod");
    expect(where().get("sel")).toBe("n001");
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("complementary", { name: "Details" })).toBeNull());
    expect(where().get("sel")).toBeNull();
  });

  it("opens straight from ?sel=", async () => {
    show("admin", "/fleet?q=n00&sel=n001");
    expect(await screen.findByRole("heading", { name: "n001" })).toBeInTheDocument();
  });

  it("a tag in the inspector becomes a filter and keeps the tags already there", async () => {
    show("admin", "/fleet?q=n00&tag=env");
    await userEvent.click(await row());
    await userEvent.click(await screen.findByRole("button", { name: "env=prod" }));
    await waitFor(() => expect(where().getAll("tag")).toEqual(["env", "env=prod"]));
  });

  it("Drain does not call the API until confirmed; Undrain is restorative", async () => {
    show("admin", "/fleet?q=n00");
    await userEvent.click(await row());
    const panel = await screen.findByRole("complementary", { name: "Details" });
    await userEvent.click(within(panel).getByRole("button", { name: "Drain" }));
    expect(api.drainInstance).not.toHaveBeenCalled();
    const dlg = screen.getByRole("dialog");
    expect(dlg).toHaveTextContent("n001");
    await userEvent.click(within(dlg).getByRole("button", { name: "Cancel" }));
    expect(api.drainInstance).not.toHaveBeenCalled();
    await userEvent.click(within(panel).getByRole("button", { name: "Drain" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Drain" }));
    expect(api.drainInstance).toHaveBeenCalledWith("n001");
    await userEvent.click(within(panel).getByRole("button", { name: "Undrain" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(api.undrainInstance).toHaveBeenCalledWith("n001");
  });

  it("remove and disable of a backend warn they are fleet-wide and wait for confirmation", async () => {
    show("admin", "/fleet?q=n00");
    await userEvent.click(await row());
    const panel = await screen.findByRole("complementary", { name: "Details" });
    await userEvent.click(within(panel).getByRole("button", { name: "remove" }));
    expect(api.deleteBackend).not.toHaveBeenCalled();
    const dlg = screen.getByRole("dialog");
    expect(dlg).toHaveTextContent(/every instance/i);
    await userEvent.click(within(dlg).getByRole("button", { name: "Remove" }));
    expect(api.deleteBackend).toHaveBeenCalledWith("lobby", "10.0.0.5:7777");

    await userEvent.selectOptions(within(panel).getByLabelText("State of 10.0.0.5:7777"), "disabled");
    expect(api.patchBackend).not.toHaveBeenCalled();
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Disable" }));
    expect(api.patchBackend).toHaveBeenCalledWith("lobby", "10.0.0.5:7777", "disabled");
  });

  it("reports a failure and turns the buttons back on", async () => {
    let reject!: (e: Error) => void;
    api.undrainInstance.mockReturnValue(new Promise((_, rej) => (reject = rej)));
    show("admin", "/fleet?q=n00");
    await userEvent.click(await row());
    const panel = await screen.findByRole("complementary", { name: "Details" });
    await userEvent.click(within(panel).getByRole("button", { name: "Undrain" }));
    expect(within(panel).getByRole("button", { name: "Drain" })).toHaveAttribute("aria-disabled", "true");
    reject(new Error("aggregator unreachable"));
    expect(await screen.findByText(/undrain n001 failed: .*aggregator unreachable/)).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: "Drain" })).not.toHaveAttribute("aria-disabled", "true");
  });

  it("a viewer can look but not act", async () => {
    show("viewer", "/fleet?q=n00");
    await userEvent.click(await row());
    const panel = await screen.findByRole("complementary", { name: "Details" });
    expect(within(panel).getByRole("button", { name: "Drain" })).toHaveAttribute("aria-disabled", "true");
    expect(within(panel).getByRole("button", { name: "remove" })).toHaveAttribute("aria-disabled", "true");
  });
});

describe("Fleet page bulk actions", () => {
  it("resolves the filter, states the count, and drains each node only after confirming", async () => {
    show("admin", "/fleet?state=degraded&page=2&sel=");
    await screen.findByText(/Matching: 20 nodes/);
    await userEvent.click(screen.getByRole("button", { name: "Drain all matching…" }));
    const dlg = await screen.findByRole("dialog");
    expect(api.resolveFleetNodes).toHaveBeenCalledWith("state=degraded"); // the filter only: no paging
    expect(dlg).toHaveTextContent("Drain 20 nodes?");
    expect(dlg).toHaveTextContent("20 nodes match the current filter");
    expect(api.drainInstance).not.toHaveBeenCalled();
    await userEvent.click(within(dlg).getByRole("button", { name: "Drain 20" }));
    await waitFor(() => expect(api.drainInstance).toHaveBeenCalledTimes(20));
    expect(await screen.findByText("drain: 20 nodes done")).toBeInTheDocument();
  });

  it("cancelling touches nothing, and an unfiltered bulk says it is the whole fleet", async () => {
    show();
    await screen.findByText("Showing 1 to 25 of 500");
    await userEvent.click(screen.getByRole("button", { name: "Undrain whole fleet…" }));
    const dlg = await screen.findByRole("dialog");
    expect(dlg).toHaveTextContent("500 nodes match the current filter (the whole fleet)");
    await userEvent.click(within(dlg).getByRole("button", { name: "Cancel" }));
    expect(api.undrainInstance).not.toHaveBeenCalled();
  });

  it("names the nodes that failed, so they can be retried", async () => {
    api.drainInstance.mockImplementation(async (i: string) => {
      if (i === "n099" || i === "n299") throw new Error("boom");
      return "ok";
    });
    show("admin", "/fleet?state=stale");
    await screen.findByText(/Matching: 5 nodes/);
    await userEvent.click(screen.getByRole("button", { name: "Drain all matching…" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Drain 5" }));
    expect(await screen.findByText(/drain: 3 of 5 done, 2 failed \(n099, n299\)/)).toBeInTheDocument();
  });

  it("lists every failure with its reason and retries only those", async () => {
    let down = true;
    api.drainInstance.mockImplementation(async (i: string) => {
      if (down && i === "n099") throw new Error("aggregator: 502");
      return "ok";
    });
    show("admin", "/fleet?state=stale");
    await screen.findByText(/Matching: 5 nodes/);
    await userEvent.click(screen.getByRole("button", { name: "Drain all matching…" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Drain 5" }));
    const block = await screen.findByRole("region", { name: "Failed nodes" });
    expect(block).toHaveTextContent("n099: aggregator: 502");
    expect(block).not.toHaveTextContent("n199");
    api.drainInstance.mockClear();
    down = false;
    await userEvent.click(within(block).getByRole("button", { name: "Retry failed" }));
    await waitFor(() => expect(api.drainInstance.mock.calls.map((c) => c[0])).toEqual(["n099"]));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Failed nodes" })).toBeNull());
  });

  it("caps the failed names at five", async () => {
    api.drainInstance.mockRejectedValue(new Error("boom"));
    show("admin", "/fleet?state=degraded");
    await screen.findByText(/Matching: 20 nodes/);
    await userEvent.click(screen.getByRole("button", { name: "Drain all matching…" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Drain 20" }));
    expect(await screen.findByText(/20 failed \(.*and 15 more\)/)).toBeInTheDocument();
  });

  it("runs ten at a time and keeps both bulk buttons off until it is done", async () => {
    let running = 0;
    let peak = 0;
    const release: (() => void)[] = [];
    api.drainInstance.mockImplementation(
      () =>
        new Promise((resolve) => {
          running++;
          peak = Math.max(peak, running);
          release.push(() => (running--, resolve("ok")));
        }),
    );
    show("admin", "/fleet?state=degraded");
    await screen.findByText(/Matching: 20 nodes/);
    await userEvent.click(screen.getByRole("button", { name: "Drain all matching…" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Drain 20" }));
    await waitFor(() => expect(release.length).toBe(10));
    expect(screen.getByText("Draining 0 of 20…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Drain all matching…" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "Undrain all matching…" })).toHaveAttribute("aria-disabled", "true");
    while (release.length) release.shift()!();
    await waitFor(() => expect(api.drainInstance).toHaveBeenCalledTimes(20));
    while (release.length < 10) await new Promise((r) => setTimeout(r, 5));
    while (release.length) release.shift()!();
    expect(await screen.findByText("drain: 20 nodes done")).toBeInTheDocument();
    expect(peak).toBeLessThanOrEqual(10);
    expect(screen.getByRole("button", { name: "Drain all matching…" })).not.toHaveAttribute("aria-disabled", "true");
  });

  it("a viewer has no bulk actions", async () => {
    show("viewer");
    await screen.findByText("Showing 1 to 25 of 500");
    expect(screen.queryByRole("button", { name: /all matching/ })).toBeNull();
  });
});

describe("Fleet page saved filters", () => {
  it("applies a saved filter and saves the current one under a name", async () => {
    api.listFilters.mockResolvedValue([{ name: "prod gpus", tags: ["env=prod", "gpu"], region: "eu" }]);
    show();
    await screen.findByText("Showing 1 to 25 of 500");
    await screen.findByRole("option", { name: "prod gpus" });
    await userEvent.selectOptions(screen.getByLabelText("Saved filters"), "prod gpus");
    await waitFor(() => expect(where().getAll("tag")).toEqual(["env=prod", "gpu"]));
    expect(where().get("region")).toBe("eu");

    await userEvent.click(screen.getByRole("button", { name: "Save current filter…" }));
    // The name starts as the applied filter's, so saving again overwrites it.
    await userEvent.clear(screen.getByLabelText("Filter name"));
    await userEvent.type(screen.getByLabelText("Filter name"), "eu prod{Enter}");
    await waitFor(() => expect(api.saveFilter).toHaveBeenCalled());
    expect(api.saveFilter.mock.calls[0][0]).toEqual({ name: "eu prod", region: "eu", tags: ["env=prod", "gpu"] });
  });

  it("deleting asks first", async () => {
    api.listFilters.mockResolvedValue([{ name: "eu", region: "eu" }]);
    show();
    await screen.findByRole("option", { name: "eu" });
    await userEvent.selectOptions(screen.getByLabelText("Saved filters"), "eu");
    await userEvent.click(screen.getByRole("button", { name: "Delete filter" }));
    expect(api.deleteFilter).not.toHaveBeenCalled();
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(api.deleteFilter).toHaveBeenCalled());
    expect(api.deleteFilter.mock.calls[0][0]).toBe("eu");
  });

  it("a viewer can apply a saved filter but not save or delete", async () => {
    api.listFilters.mockResolvedValue([{ name: "eu", region: "eu" }]);
    show("viewer");
    await screen.findByRole("option", { name: "eu" });
    await userEvent.selectOptions(screen.getByLabelText("Saved filters"), "eu");
    expect(screen.queryByRole("button", { name: /Save current filter/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete filter" })).toBeNull();
  });
});
