import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";

const api = vi.hoisted(() => ({
  uploadPluginModule: vi.fn(),
  resolvePluginCapabilities: vi.fn(),
  installPlugin: vi.fn(),
  listPluginRegistries: vi.fn(),
  listRegistryPlugins: vi.fn(),
  fetchRegistryPlugin: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { PluginInstallPage, suggestName } from "./PluginInstallPage";

const SHA = "ab".repeat(32);
const CAPS = { triggers: { on_timer: true }, tick_interval_secs: 60, log: true, state: null };
const PANEL = { ...CAPS, http: { hosts: [{ host: "p.example" }] } };
const MANIFEST = {
  name: "bound",
  bindings: [{ option: "panel", kind: "hostname" }],
  options: { type: "object", required: ["panel"], properties: { panel: { type: "string", title: "Panel host" } } },
};

function wasm(name = "Pelican_Sync.wasm") {
  return new File([new Uint8Array([0, 97, 115, 109])], name, { type: "application/wasm" });
}

function page() {
  render(
    <MemoryRouter initialEntries={["/plugins/new"]}>
      <Routes>
        <Route path="/plugins/new" element={<PluginInstallPage />} />
        <Route path="/plugins" element={<p>plugin list</p>} />
      </Routes>
    </MemoryRouter>,
  );
}

async function upload(file = wasm()) {
  page();
  await userEvent.upload(screen.getByLabelText(/\.wasm file/), file);
  await userEvent.click(screen.getByRole("button", { name: "Inspect module" }));
}

beforeEach(() => {
  api.uploadPluginModule.mockReset().mockResolvedValue({ sha256: SHA, size: 2048, abi: "0.2", capabilities: CAPS, manifest: null });
  api.resolvePluginCapabilities.mockReset().mockResolvedValue({ effective: CAPS });
  api.installPlugin.mockReset().mockResolvedValue({});
  const reg = { id: "r1", name: "wayhouse plugins", url: "https://example.test/index.json", official: true, risk: "official" };
  api.listPluginRegistries.mockReset().mockResolvedValue({ registries: [reg], persistent: true });
  api.listRegistryPlugins.mockReset().mockResolvedValue({
    registry: reg,
    index_name: "plugins",
    host_abi: "0.2",
    plugins: [{ name: "pelican-sync", description: "Syncs", license: "MIT", compatible: true, versions: [{ version: "0.2.0", abi: "0.2", size: 2048, compatible: true, verifiable: false }] }],
  });
  api.fetchRegistryPlugin.mockReset();
});

describe("suggestName", () => {
  it("makes a plugin name of a file name", () => {
    expect(suggestName("Pelican_Sync.wasm")).toBe("pelican-sync");
  });
});

describe("PluginInstallPage without options", () => {
  it("goes straight to the review and installs exactly the resolved set", async () => {
    await upload();
    await userEvent.click(await screen.findByRole("button", { name: "Install…" }));
    const dlg = screen.getByRole("dialog");
    expect(dlg).toHaveTextContent(/runs every 60 s/i);
    expect(screen.getByLabelText("Name")).toHaveValue("pelican-sync");
    const go = within(dlg).getByRole("button", { name: "Approve and install" });
    expect(go).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(within(dlg).getByRole("checkbox", { name: /I approve/i }));
    await userEvent.click(go);
    expect(api.installPlugin).toHaveBeenCalledWith({ name: "pelican-sync", sha256: SHA, approved: CAPS, config: {}, enabled: true });
    expect(await screen.findByText("plugin list")).toBeInTheDocument();
  });

  it("installs nothing before the approval", async () => {
    await upload();
    await screen.findByRole("button", { name: "Install…" });
    expect(api.installPlugin).not.toHaveBeenCalled();
  });

  it("shows the controller's refusal inside the prompt", async () => {
    api.installPlugin.mockRejectedValue(new ApiError(422, "capability not approved: log"));
    await upload();
    await userEvent.click(await screen.findByRole("button", { name: "Install…" }));
    const dlg = screen.getByRole("dialog");
    await userEvent.click(within(dlg).getByRole("checkbox", { name: /I approve/i }));
    await userEvent.click(within(dlg).getByRole("button", { name: "Approve and install" }));
    expect(await within(dlg).findByText(/capability not approved: log/)).toBeInTheDocument();
  });

  it("refuses a file over 8 MiB without uploading it", async () => {
    page();
    const big = new File([new Uint8Array(1)], "big.wasm");
    Object.defineProperty(big, "size", { value: 8 * 1024 * 1024 + 1 });
    await userEvent.upload(screen.getByLabelText(/\.wasm file/), big);
    await userEvent.click(screen.getByRole("button", { name: "Inspect module" }));
    expect(await screen.findByText(/limited to 8 MiB/)).toBeInTheDocument();
    expect(api.uploadPluginModule).not.toHaveBeenCalled();
  });

  it("shows an upload error from the controller", async () => {
    api.uploadPluginModule.mockRejectedValue(new ApiError(400, "not a plugin module"));
    await upload();
    expect(await screen.findByText(/not a plugin module/)).toBeInTheDocument();
  });
});

describe("PluginInstallPage from a registry", () => {
  const fetched = (signed: boolean) => ({
    plugin: "pelican-sync",
    version: "0.2.0",
    signed,
    risk: "official",
    upload: { sha256: SHA, size: 2048, abi: "0.2", capabilities: CAPS, manifest: null },
  });
  async function fromRegistry() {
    page();
    await userEvent.click(screen.getByRole("button", { name: "From a registry" }));
    await userEvent.click(await screen.findByRole("button", { name: "Fetch pelican-sync" }));
  }

  it("lands on the review with the fetched module, says where it came from, and installs nothing yet", async () => {
    api.fetchRegistryPlugin.mockResolvedValue(fetched(true));
    await fromRegistry();
    await screen.findByRole("button", { name: "Install…" });
    expect(screen.getByText(/From wayhouse plugins/)).toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toHaveValue("pelican-sync");
    expect(api.installPlugin).not.toHaveBeenCalled();
    expect(api.uploadPluginModule).not.toHaveBeenCalled();
  });

  it("still shows the approval prompt and installs the fetched module's hash", async () => {
    api.fetchRegistryPlugin.mockResolvedValue(fetched(true));
    await fromRegistry();
    await userEvent.click(await screen.findByRole("button", { name: "Install…" }));
    const dlg = screen.getByRole("dialog");
    await userEvent.click(within(dlg).getByRole("checkbox", { name: /I approve/i }));
    await userEvent.click(within(dlg).getByRole("button", { name: "Approve and install" }));
    expect(api.installPlugin).toHaveBeenCalledWith(expect.objectContaining({ name: "pelican-sync", sha256: SHA }));
  });

  it("does not claim a signature the controller did not check", async () => {
    api.fetchRegistryPlugin.mockResolvedValue(fetched(false));
    await fromRegistry();
    await screen.findByRole("button", { name: "Install…" });
    expect(screen.getByText(/signature was not verified/)).toBeInTheDocument();
    expect(screen.queryByText(/no registry vouches/)).toBeNull();
  });
});

describe("PluginInstallPage with options", () => {
  beforeEach(() => {
    api.uploadPluginModule.mockResolvedValue({ sha256: SHA, size: 2048, abi: "0.2", capabilities: CAPS, manifest: MANIFEST });
    api.resolvePluginCapabilities.mockResolvedValue({ effective: PANEL });
  });

  it("asks for the options, shows what they resolve to and installs that set with the config", async () => {
    await upload();
    await userEvent.type(await screen.findByLabelText(/Panel host/), "p.example");
    await userEvent.click(screen.getByRole("button", { name: "Review what it asks for" }));
    expect(api.resolvePluginCapabilities).toHaveBeenCalledWith(expect.objectContaining({ sha256: SHA, config: { panel: "p.example" } }));
    await userEvent.click(await screen.findByRole("button", { name: "Install…" }));
    const dlg = screen.getByRole("dialog");
    expect(dlg).toHaveTextContent("Makes HTTPS requests to p.example");
    await userEvent.click(within(dlg).getByRole("checkbox", { name: /I approve/i }));
    await userEvent.click(within(dlg).getByRole("button", { name: "Approve and install" }));
    expect(api.installPlugin).toHaveBeenCalledWith(expect.objectContaining({ approved: PANEL, config: { panel: "p.example" } }));
  });

  it("moves focus to the new step when the step changes, so a keyboard or screen reader user lands on it", async () => {
    await upload();
    await screen.findByLabelText(/Panel host/);
    expect(document.activeElement).toHaveTextContent("2. Options");
    await userEvent.type(screen.getByLabelText(/Panel host/), "p.example");
    await userEvent.click(screen.getByRole("button", { name: "Review what it asks for" }));
    await screen.findByRole("button", { name: "Install…" });
    expect(document.activeElement).toHaveTextContent("3. Review and install");
  });

  it("names the option that does not fit and stays on the options", async () => {
    api.resolvePluginCapabilities.mockRejectedValue(new ApiError(422, "config.panel: must be a hostname"));
    await upload();
    await userEvent.type(await screen.findByLabelText(/Panel host/), "x");
    await userEvent.click(screen.getByRole("button", { name: "Review what it asks for" }));
    expect(await screen.findByText(/config\.panel: must be a hostname/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Install…" })).not.toBeInTheDocument();
  });

  it("shows a changed set and asks again when the install finds the resolved set moved", async () => {
    const MOVED = { ...PANEL, http: { hosts: [{ host: "p.example" }, { host: "extra.example" }] } };
    api.installPlugin.mockRejectedValueOnce(new ApiError(400, "approved must be exactly what the options resolve to", { effective: MOVED }));
    await upload();
    await userEvent.type(await screen.findByLabelText(/Panel host/), "p.example");
    await userEvent.click(screen.getByRole("button", { name: "Review what it asks for" }));
    await userEvent.click(await screen.findByRole("button", { name: "Install…" }));
    const dlg = screen.getByRole("dialog");
    await userEvent.click(within(dlg).getByRole("checkbox", { name: /I approve/i }));
    await userEvent.click(within(dlg).getByRole("button", { name: "Approve and install" }));
    expect(await within(dlg).findByText(/extra\.example/)).toBeInTheDocument();
    expect(within(dlg).getByRole("checkbox", { name: /I approve/i })).not.toBeChecked();
    expect(api.installPlugin).toHaveBeenCalledTimes(1);
  });

  it("says secret options are set after the install", async () => {
    api.uploadPluginModule.mockResolvedValue({
      sha256: SHA,
      size: 1,
      abi: "0.2",
      capabilities: CAPS,
      manifest: {
        ...MANIFEST,
        options: { type: "object", properties: { panel: { type: "string" }, api_key: { type: "string", "x-wayhouse-secret": true } } },
      },
    });
    await upload();
    expect(await screen.findByText(/api_key is a secret/)).toBeInTheDocument();
    expect(screen.queryByLabelText(/api_key/)).not.toBeInTheDocument();
  });
});
