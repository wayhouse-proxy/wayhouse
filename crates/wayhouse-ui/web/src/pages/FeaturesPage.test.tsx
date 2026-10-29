import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import { SessionProvider, type Role } from "../lib/session";

const api = vi.hoisted(() => ({
  getFeatures: vi.fn(),
  listFeatureOverrides: vi.fn(),
  putFeatureOverride: vi.fn(),
  clearFeatureOverride: vi.fn(),
  getCurrentConfig: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { FeaturesPage } from "./FeaturesPage";

const NOW = 1_800_000_000;

function flag(name: string, over: Record<string, unknown> = {}) {
  return {
    name,
    value: true,
    source: "default",
    default: true,
    scope: "proxy",
    apply: "live",
    override: "both",
    protection: false,
    wired: true,
    available: true,
    since: 4,
    risk: `${name} risk`,
    impact: `${name} impact`,
    ...over,
  };
}

const page = (role: Role = "admin") =>
  render(
    <SessionProvider value={{ username: "ana", role }}>
      <FeaturesPage />
    </SessionProvider>,
  );

const row = (name: string) => screen.getByText(name, { selector: "td" }).closest("tr")!;

beforeEach(() => {
  vi.spyOn(Date, "now").mockReturnValue(NOW * 1000);
  Object.values(api).forEach((f) => f.mockReset());
  api.getFeatures.mockResolvedValue({
    flags: [
      flag("plugin_webhooks", { scope: "controller", override: "off_only" }),
      flag("acme_issuance", { scope: "controller", value: false, source: "build", available: false }),
    ],
    proxy_flags: [flag("sniffers"), flag("admin_write.route_hint"), flag("acl", { protection: true }), flag("port_routes", { override: "off_only" }), flag("geo", { protection: true, wired: false })],
  });
  api.listFeatureOverrides.mockResolvedValue({ items: [] });
  api.getCurrentConfig.mockResolvedValue({ text: "features:\n  admin_write.route_hint: false\n", revision: "3" });
  api.putFeatureOverride.mockResolvedValue({});
  api.clearFeatureOverride.mockResolvedValue(undefined);
});

describe("Features page", () => {
  it("lists the flags with their value, source and risk, and the config value beats the default", async () => {
    page();
    expect(await screen.findByText("plugin_webhooks", { selector: "td" })).toBeInTheDocument();
    const sniffers = row("admin_write.route_hint");
    expect(within(sniffers).getByText("off")).toBeInTheDocument();
    expect(within(sniffers).getByText("config")).toBeInTheDocument();
    expect(within(sniffers).getByText("admin_write.route_hint risk")).toBeInTheDocument();
    expect(within(row("acl")).getByText("on")).toBeInTheDocument();
    expect(within(row("acl")).getByText("default")).toBeInTheDocument();
  });

  it("explains a flag the build lacks instead of offering to change it", async () => {
    page();
    await screen.findByText("acme_issuance", { selector: "td" });
    const r = row("acme_issuance");
    expect(within(r).getByText("not in this build")).toBeInTheDocument();
    expect(within(r).queryByRole("button")).toBeNull();
  });

  it("names the flags that are in the registry but not enforced yet, and gives them no switch", async () => {
    page();
    await screen.findByText("sniffers", { selector: "td" });
    expect(screen.queryByText("geo", { selector: "td" })).toBeNull();
    expect(screen.getByText(/not enforced by this build yet/)).toHaveTextContent("geo");
  });

  it("an override wins over the config value and shows its reason, who set it and when it ends", async () => {
    api.listFeatureOverrides.mockResolvedValue({ items: [{ name: "sniffers", value: true, reason: "sniffer bug fixed", actor: "bob", set_at: NOW - 60, expires_at: NOW + 7200 }] });
    api.getFeatures.mockResolvedValue({
      flags: [],
      proxy_flags: [flag("sniffers", { source: "override" })],
    });
    page();
    const r = await screen.findByText("sniffers", { selector: "td" }).then(() => row("sniffers"));
    expect(within(r).getByText("override")).toBeInTheDocument();
    expect(within(r).getByText("on")).toBeInTheDocument();
    expect(within(r).getByText(/sniffer bug fixed/)).toBeInTheDocument();
    expect(within(r).getByText(/bob/)).toBeInTheDocument();
    expect(within(r).getByText(/ends in 2 h/)).toBeInTheDocument();
  });

  it("turning a flag off asks for a reason and shows what it costs and what it does to open connections", async () => {
    page();
    await userEvent.click(await screen.findByRole("button", { name: "Override sniffers" }));
    const dlg = await screen.findByRole("dialog");
    expect(within(dlg).getByText("sniffers risk")).toBeInTheDocument();
    expect(within(dlg).getByText("sniffers impact")).toBeInTheDocument();
    const go = within(dlg).getByRole("button", { name: "Turn off sniffers" });
    await userEvent.click(go);
    expect(await within(dlg).findByText(/Give a reason/)).toBeInTheDocument();
    expect(api.putFeatureOverride).not.toHaveBeenCalled();
    await userEvent.type(within(dlg).getByLabelText("Reason"), "players land on the wrong pool");
    await userEvent.click(go);
    expect(api.putFeatureOverride).toHaveBeenCalledWith("sniffers", false, "players land on the wrong pool", undefined);
  });

  it("switching a protection flag off needs an expiry and the page says so", async () => {
    page();
    await userEvent.click(await screen.findByRole("button", { name: "Override acl" }));
    const dlg = await screen.findByRole("dialog");
    expect(within(dlg).getByText(/removes protection/)).toBeInTheDocument();
    expect(within(dlg).queryByRole("option", { name: /never/ })).toBeNull();
    await userEvent.type(within(dlg).getByLabelText("Reason"), "false positives");
    await userEvent.selectOptions(within(dlg).getByLabelText("Back on after"), "3600");
    await userEvent.click(within(dlg).getByRole("button", { name: "Turn off acl" }));
    expect(api.putFeatureOverride).toHaveBeenCalledWith("acl", false, "false positives", NOW + 3600);
  });

  it("a flag that only goes off cannot be switched on by an override, but its override can be cleared", async () => {
    api.getFeatures.mockResolvedValue({
      flags: [],
      proxy_flags: [flag("port_routes", { override: "off_only", value: false, source: "override" })],
    });
    api.listFeatureOverrides.mockResolvedValue({ items: [{ name: "port_routes", value: false, reason: "exposure", set_at: NOW - 5 }] });
    page();
    await screen.findByText("port_routes", { selector: "td" });
    expect(screen.queryByRole("button", { name: "Override port_routes" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Clear override of port_routes" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Clear override" }));
    expect(api.clearFeatureOverride).toHaveBeenCalledWith("port_routes");
  });

  it("warns at the top while a protection flag is off", async () => {
    api.getFeatures.mockResolvedValue({
      flags: [],
      proxy_flags: [flag("acl", { protection: true, value: false, source: "override" })],
    });
    api.listFeatureOverrides.mockResolvedValue({ items: [{ name: "acl", value: false, reason: "r", set_at: NOW - 5, expires_at: NOW + 600 }] });
    page();
    expect(await screen.findByText(/Protection is reduced: acl is off/)).toBeInTheDocument();
  });

  it("shows the controller's refusal inside the dialog", async () => {
    api.putFeatureOverride.mockRejectedValue(new ApiError(422, "unknown flag"));
    page();
    await userEvent.click(await screen.findByRole("button", { name: "Override sniffers" }));
    const dlg = await screen.findByRole("dialog");
    await userEvent.type(within(dlg).getByLabelText("Reason"), "x");
    await userEvent.click(within(dlg).getByRole("button", { name: "Turn off sniffers" }));
    expect(await within(dlg).findByText(/unknown flag/)).toBeInTheDocument();
  });

  it("is read-only below admin", async () => {
    page("operator");
    await screen.findByText("sniffers", { selector: "td" });
    expect(screen.getByRole("button", { name: "Override sniffers" })).toHaveAttribute("aria-disabled", "true");
  });

  it("still lists the flags when the config cannot be read", async () => {
    api.getCurrentConfig.mockRejectedValue(new ApiError(500, "down"));
    page();
    expect(await screen.findByText("sniffers", { selector: "td" })).toBeInTheDocument();
    expect(screen.getByText(/config values are not shown/)).toBeInTheDocument();
  });
});
