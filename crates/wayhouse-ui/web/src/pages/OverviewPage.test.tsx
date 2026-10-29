import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const api = vi.hoisted(() => ({ getFleetStats: vi.fn() }));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));
import { OverviewPage } from "./OverviewPage";
import { ListenersPage } from "./ListenersPage";
import { PoolsPage } from "./PoolsPage";
import { onset, freshness, fmtBytes, fmtRate } from "../lib/stats";

const T0 = Date.UTC(2026, 9, 29, 12, 0);
const pt = (i: number, over = {}) => ({ t_ms: T0 + i * 60_000, partial: false, accepted_per_sec: 10 + i, blocked_per_sec: i < 2 ? 0 : 3, bytes_per_sec: 1500 * (i + 1), packets_per_sec: 5, tcp_sessions: 20 + i, udp_sessions: i === 1 ? null : 4, ...over });
const stats = {
  generated_at_ms: T0 + 3 * 60_000 + 20_000,
  range_sec: 900,
  fleet: { resolution_sec: 60, last_sample_ms: T0 + 3 * 60_000, points: [0, 1, 2, 3].map((i) => pt(i)) },
  listeners: [
    { name: "mc", resolution_sec: 60, last_sample_ms: T0, points: [0, 1].map((i) => ({ t_ms: T0 + i * 60_000, partial: false, accepted_per_sec: 4, blocked_per_sec: 1, bytes_c2s_per_sec: 100, bytes_s2c_per_sec: 200, packets_per_sec: 1 })) },
    { name: "dns", resolution_sec: 60, last_sample_ms: T0, points: [{ t_ms: T0, partial: false, accepted_per_sec: 9, blocked_per_sec: 0, bytes_c2s_per_sec: 1, bytes_s2c_per_sec: 2, packets_per_sec: 1 }] },
  ],
  pools: [{ name: "eu", resolution_sec: 60, last_sample_ms: T0, points: [{ t_ms: T0, partial: false, healthy_backends: 1, backends: 3 }] }],
  regions: [],
};

function show(ui: React.ReactElement, path = "/") {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[path]}>{ui}</MemoryRouter>
    </QueryClientProvider>,
  );
}
beforeEach(() => {
  api.getFleetStats.mockReset().mockResolvedValue(stats);
});

describe("Overview", () => {
  it("states what each chart shows, with the latest values, for a screen reader", async () => {
    show(<OverviewPage />);
    const chart = await screen.findByRole("img", { name: /^Connections\. At 12:03 UTC: Accepted 13\/s, Blocked 3\/s\. Blocking began at 12:02 UTC\.$/ });
    expect(chart).toBeInTheDocument();
    expect(screen.getByText(/Last sample 20 s ago/)).toBeInTheDocument();
  });

  it("the table view holds exactly the chart's data, newest first, with a gap for a missing sample", async () => {
    show(<OverviewPage />);
    await screen.findByRole("img", { name: /^Sessions/ });
    const toggles = screen.getAllByRole("button", { name: "View as table" });
    await userEvent.click(toggles[2]);
    const table = screen.getByRole("table", { name: "Sessions, as a table" });
    const rows = within(table).getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("cell").map((c) => c.textContent));
    expect(rows).toEqual([["12:03", "23", "4"], ["12:02", "22", "4"], ["12:01", "21", "–"], ["12:00", "20", "4"]]);
  });

  it("asks for the range in the URL", async () => {
    show(<OverviewPage />, "/?range=1h");
    await screen.findByRole("img", { name: /^Connections/ });
    expect(api.getFleetStats).toHaveBeenCalledWith("1h");
    expect(screen.getByRole("combobox", { name: "Time range" })).toHaveValue("1h");
  });

  it("says so when no instance has reported", async () => {
    api.getFleetStats.mockResolvedValue({ ...stats, fleet: { resolution_sec: 60, last_sample_ms: null, points: [] }, listeners: [], pools: [] });
    show(<OverviewPage />);
    expect(await screen.findByText(/No instance has reported yet/)).toBeInTheDocument();
  });

  it("shows the error and a retry when the aggregator is unreachable", async () => {
    api.getFleetStats.mockRejectedValue(new Error("aggregator not configured"));
    show(<OverviewPage />);
    expect(await screen.findByText(/aggregator not configured/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});

describe("Listeners and pools", () => {
  it("lists every listener with its latest rate and charts the one chosen in the URL", async () => {
    show(<ListenersPage />, "/?name=dns");
    expect(await screen.findByRole("heading", { name: "dns" })).toBeInTheDocument();
    const row = screen.getByRole("link", { name: "mc" }).closest("tr")!;
    expect(within(row).getByText("4/s")).toBeInTheDocument();
    expect(within(row).getByText("300 B/s")).toBeInTheDocument();
  });

  it("a pool with a missing backend is degraded, in words", async () => {
    show(<PoolsPage />);
    const row = (await screen.findByRole("link", { name: "eu" })).closest("tr")!;
    expect(within(row).getByText("Degraded")).toBeInTheDocument();
  });
});

describe("stats helpers", () => {
  it("finds when a rate began and ignores one that was always on", () => {
    const v = (p: { v: number }) => p.v;
    expect(onset([{ v: 0 }, { v: 0 }, { v: 2 }, { v: 3 }], v, () => 7)).toBe(7);
    expect(onset([{ v: 1 }, { v: 2 }], v, () => 7)).toBeUndefined();
  });
  it("states freshness and stale data", () => {
    expect(freshness(100_000, 90_000)).toEqual({ text: "Last sample 10 s ago", stale: false });
    expect(freshness(1_000_000, 100_000).stale).toBe(true);
    expect(freshness(1, null).stale).toBe(true);
  });
  it("formats rates and bytes", () => {
    expect(fmtRate(0)).toBe("0/s");
    expect(fmtRate(12.34)).toBe("12.3/s");
    expect(fmtBytes(1_500_000)).toBe("1.5 MB/s");
  });
});
