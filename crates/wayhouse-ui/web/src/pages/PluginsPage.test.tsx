import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";

const api = vi.hoisted(() => ({
  listPlugins: vi.fn(),
  listPluginRoutes: vi.fn(),
  setPluginEnabled: vi.fn(),
  deletePlugin: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { PluginsPage } from "./PluginsPage";

const NOW = 1_800_000_000;
const CAPS = {
  triggers: { on_timer: true },
  tick_interval_secs: 60,
  log: true,
  state: { max_bytes: 65536 },
};
const SHA = "ab".repeat(32);

function install(extra = {}) {
  return {
    id: "0123456789abcdef",
    name: "pelican-sync",
    sha256: SHA,
    size: 2048,
    approved: CAPS,
    config: {},
    enabled: true,
    created_at: NOW - 3600,
    created_by: "alice",
    ...extra,
  };
}

beforeEach(() => {
  vi.spyOn(Date, "now").mockReturnValue(NOW * 1000);
  api.listPlugins.mockReset().mockResolvedValue([install()]);
  api.listPluginRoutes.mockReset().mockResolvedValue({ routes: [], conflicts: [] });
  api.setPluginEnabled.mockReset().mockResolvedValue(install({ enabled: false }));
  api.deletePlugin.mockReset().mockResolvedValue(undefined);
});

describe("PluginsPage list", () => {
  it("shows each install with its approved capabilities in words", async () => {
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    const row = (await screen.findByText("pelican-sync")).closest("tr")!;
    expect(within(row).getByText("enabled")).toBeInTheDocument();
    expect(within(row).getByText(/runs every 60 s/i)).toBeInTheDocument();
    expect(within(row).getByText(/writes log lines/i)).toBeInTheDocument();
    expect(within(row).getByText(/keeps up to 64 KiB of state/i)).toBeInTheDocument();
    expect(within(row).getByText("abababababab…")).toBeInTheDocument();
  });

  it("shows the controller's reason when plugins are not served", async () => {
    api.listPlugins.mockRejectedValue(
      new ApiError(501, "plugins are off; start the controller with --plugins"),
    );
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    expect(await screen.findByText(/start the controller with --plugins/)).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("links each install to its page and offers the install wizard", async () => {
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    expect(await screen.findByRole("link", { name: "pelican-sync" })).toHaveAttribute("href", "/plugins/0123456789abcdef");
    expect(screen.getByRole("link", { name: "Install plugin" })).toHaveAttribute("href", "/plugins/new");
  });

  it("says so when there are no installs", async () => {
    api.listPlugins.mockResolvedValue([]);
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    expect(await screen.findByText(/No plugins installed/)).toBeInTheDocument();
  });
});

const PORT_CAPS = {
  ...CAPS,
  port_routes: {
    ports: ["30000-30099"],
    proto: ["udp", "tcp"],
    backends: ["10.0.0.0/16"],
    max_entries: 256,
  },
};

describe("PluginsPage port routes", () => {
  it("lists the entry port first and says which plugin and backend it serves", async () => {
    api.listPluginRoutes.mockResolvedValue({
      routes: [],
      conflicts: [],
      port_routes: [
        { key: "srv-b", port: 30007, proto: "both", backend: "10.0.1.3:25565", owner: "plugin:1", plugin: "pelican-sync" },
        { key: "srv-a", port: 30005, proto: "udp", backend: "10.0.1.2:7777", owner: "plugin:1", plugin: "pelican-sync" },
      ],
    });
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    const heading = await screen.findByRole("heading", { name: "Port routes" });
    const table = heading.parentElement!.querySelector("table")!;
    const rows = within(table).getAllByRole("row");
    expect(within(rows[0]).getAllByRole("columnheader")[0]).toHaveTextContent(/entry port/i);
    expect(within(rows[1]).getAllByRole("cell")[0]).toHaveTextContent("30005");
    expect(rows[1]).toHaveTextContent("srv-a");
    expect(rows[1]).toHaveTextContent("udp");
    expect(rows[1]).toHaveTextContent("10.0.1.2:7777");
    expect(rows[1]).toHaveTextContent("pelican-sync");
    expect(rows[2]).toHaveTextContent("udp and tcp");
  });

  it("warns about routes that wait for a free port", async () => {
    api.listPluginRoutes.mockResolvedValue({
      routes: [],
      conflicts: [],
      port_routes: [],
      exhausted: [{ key: "srv-z", backend: "10.0.1.9:1", owner: "plugin:1", plugin: "pelican-sync" }],
    });
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    expect(await screen.findByText(/no free entry port/i)).toHaveTextContent("srv-z");
  });

  it("shows no port routes section without port routes or on a controller that has no overlay", async () => {
    api.listPluginRoutes.mockRejectedValue(new ApiError(404, "not found"));
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    await screen.findByText("pelican-sync");
    expect(screen.queryByRole("heading", { name: "Port routes" })).not.toBeInTheDocument();
  });

  it("describes a port routes approval in words, ports and networks included", async () => {
    api.listPlugins.mockResolvedValue([install({ approved: PORT_CAPS })]);
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    const row = (await screen.findByText("pelican-sync")).closest("tr")!;
    expect(within(row).getByText(/entry ports 30000-30099 \(udp, tcp\)/i)).toBeInTheDocument();
    expect(within(row).getByText(/10\.0\.0\.0\/16/)).toBeInTheDocument();
    expect(within(row).queryByText(/also asks for/i)).not.toBeInTheDocument();
  });
});

describe("PluginsPage enable, disable and delete", () => {
  it("disables an enabled plugin and enables a disabled one", async () => {
    api.listPlugins.mockResolvedValueOnce([install()]).mockResolvedValue([install({ enabled: false })]);
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    await userEvent.click(await screen.findByRole("button", { name: "Disable pelican-sync" }));
    expect(api.setPluginEnabled).not.toHaveBeenCalled();
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Disable" }));
    expect(api.setPluginEnabled).toHaveBeenCalledWith("0123456789abcdef", false);

    await userEvent.click(await screen.findByRole("button", { name: "Enable pelican-sync" }));
    expect(api.setPluginEnabled).toHaveBeenLastCalledWith("0123456789abcdef", true);
  });

  it("does not disable when the confirmation is cancelled", async () => {
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    await userEvent.click(await screen.findByRole("button", { name: "Disable pelican-sync" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));
    expect(api.setPluginEnabled).not.toHaveBeenCalled();
  });

  it("asks before deleting and only then deletes", async () => {
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    await userEvent.click(await screen.findByRole("button", { name: "Delete pelican-sync" }));
    expect(api.deletePlugin).not.toHaveBeenCalled();
    const dlg = screen.getByRole("dialog");
    expect(dlg).toHaveTextContent("pelican-sync");
    await userEvent.click(within(dlg).getByRole("button", { name: "Delete" }));
    expect(api.deletePlugin).toHaveBeenCalledWith("0123456789abcdef");
  });

  it("shows a failed action's message", async () => {
    api.setPluginEnabled.mockRejectedValue(new ApiError(404, "no such install"));
    render(<MemoryRouter><PluginsPage /></MemoryRouter>);
    await userEvent.click(await screen.findByRole("button", { name: "Disable pelican-sync" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Disable" }));
    expect(await screen.findByText(/no such install/)).toBeInTheDocument();
  });
});
