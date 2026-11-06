import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../../api";

const api = vi.hoisted(() => ({
  listPluginRegistries: vi.fn(),
  listRegistryPlugins: vi.fn(),
  fetchRegistryPlugin: vi.fn(),
}));
vi.mock("../../api", async (orig) => ({ ...(await orig<typeof import("../../api")>()), ...api }));

import { PluginRegistrySection } from "./PluginRegistrySection";

const SHA = "cd".repeat(32);
const MODULE = { sha256: SHA, size: 4096, abi: "0.2", capabilities: { triggers: {}, log: true }, manifest: null };
const REG = { id: "r1", name: "wayhouse plugins", url: "https://example.test/index.json", official: true, risk: "official" as const };
const EXT = { ...REG, id: "r2", name: "somebody's plugins", official: false, risk: "external" as const };

const version = (v: string, over = {}) => ({ version: v, abi: "0.2", size: 4096, compatible: true, verifiable: false, ...over });
const plugins = (registry: typeof REG | typeof EXT = REG) => ({
  registry,
  index_name: "plugins",
  host_abi: "0.2",
  plugins: [
    {
      name: "pelican-sync",
      description: "Syncs backends from Pelican",
      license: "MIT",
      compatible: true,
      versions: [
        version("0.2.0", { options_schema: { properties: { panel: { title: "Panel host", description: "where Pelican runs" } } } }),
        version("0.1.0", { abi: "0.1", compatible: false }),
      ],
    },
    { name: "old-thing", description: "Built for an older ABI", license: "MIT", compatible: false, versions: [version("0.1.0", { abi: "0.1", compatible: false })] },
  ],
});

const done = vi.fn();
const section = () => render(<PluginRegistrySection onFetched={done} />);

beforeEach(() => {
  Object.values(api).forEach((f) => f.mockReset());
  done.mockReset();
  api.listPluginRegistries.mockResolvedValue({ registries: [REG, EXT], persistent: true });
  api.listRegistryPlugins.mockImplementation(async (id: string) => plugins(id === "r2" ? EXT : REG));
  api.fetchRegistryPlugin.mockResolvedValue({ plugin: "pelican-sync", version: "0.2.0", signed: true, risk: "official", upload: MODULE });
});

describe("PluginRegistrySection", () => {
  it("shows the options a plugin takes before anything is downloaded", async () => {
    section();
    const card = (await screen.findByText("pelican-sync")).closest("li")!;
    expect(within(card).getByText("Panel host")).toBeInTheDocument();
    expect(within(card).getByText(/where Pelican runs/)).toBeInTheDocument();
    expect(api.fetchRegistryPlugin).not.toHaveBeenCalled();
  });

  it("marks a version nobody can verify as unverified, and one built for another ABI as incompatible", async () => {
    section();
    const card = (await screen.findByText("pelican-sync")).closest("li")!;
    expect(within(card).getByText("signature not verified")).toBeInTheDocument();
    const old = screen.getByText("old-thing").closest("li")!;
    expect(within(old).getByText(/plugin ABI 0\.1/)).toBeInTheDocument();
    expect(within(old).getByRole("button", { name: "Fetch old-thing" })).toHaveAttribute("aria-disabled", "true");
  });

  it("fetches the newest compatible version and hands the uploaded module on", async () => {
    const user = userEvent.setup();
    api.listRegistryPlugins.mockResolvedValue(plugins());
    api.fetchRegistryPlugin.mockResolvedValue({ plugin: "pelican-sync", version: "0.2.0", signed: true, risk: "official", upload: MODULE });
    section();
    await screen.findByText("pelican-sync");
    await user.click(screen.getByRole("button", { name: "Fetch pelican-sync" }));
    expect(api.fetchRegistryPlugin).toHaveBeenCalledWith("r1", "pelican-sync", "0.2.0", false);
    expect(done).toHaveBeenCalledWith(expect.objectContaining({ upload: MODULE, plugin: "pelican-sync", version: "0.2.0", signed: true }), REG);
  });

  it("asks before fetching a module whose signature was not verified, and sends nothing until accepted", async () => {
    const user = userEvent.setup();
    api.fetchRegistryPlugin.mockRejectedValueOnce(new ApiError(409, "this module's signature could not be verified; confirm to fetch it anyway", { needs_confirm: true, risk: "official", signed: false }));
    section();
    await screen.findByText("pelican-sync");
    await user.click(screen.getByRole("button", { name: "Fetch pelican-sync" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/signature could not be verified/)).toBeInTheDocument();
    const go = within(dialog).getByRole("button", { name: "Fetch anyway" });
    expect(go).toHaveAttribute("aria-disabled", "true");
    expect(done).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(go);
    expect(api.fetchRegistryPlugin).toHaveBeenLastCalledWith("r1", "pelican-sync", "0.2.0", true);
    expect(done).toHaveBeenCalledTimes(1);
  });

  it("names the external-registry risk for a registry nobody here vetted", async () => {
    const user = userEvent.setup();
    api.fetchRegistryPlugin.mockRejectedValueOnce(new ApiError(409, "unverified", { needs_confirm: true, risk: "external", signed: false }));
    section();
    await screen.findByText("pelican-sync");
    await user.selectOptions(screen.getByLabelText("Registry"), "r2");
    await user.click(await screen.findByRole("button", { name: "Fetch pelican-sync" }));
    expect(await screen.findByText(/external, at your own risk/)).toBeInTheDocument();
  });

  it("shows a refusal and keeps the module out", async () => {
    const user = userEvent.setup();
    api.fetchRegistryPlugin.mockRejectedValueOnce(new ApiError(422, "the module does not match its hash"));
    section();
    await screen.findByText("pelican-sync");
    await user.click(screen.getByRole("button", { name: "Fetch pelican-sync" }));
    expect(await screen.findByText(/does not match its hash/)).toBeInTheDocument();
    expect(done).not.toHaveBeenCalled();
  });

  it("says so when there is no plugin registry", async () => {
    api.listPluginRegistries.mockResolvedValue({ registries: [], persistent: true });
    section();
    expect(await screen.findByText(/No plugin registry/)).toBeInTheDocument();
  });
});
