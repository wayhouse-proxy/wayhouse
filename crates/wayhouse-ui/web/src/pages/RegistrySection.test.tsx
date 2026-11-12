import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  listRegistries: vi.fn(),
  addRegistry: vi.fn(),
  removeRegistry: vi.fn(),
  listRegistrySniffers: vi.fn(),
  getRegistrySniffer: vi.fn(),
  installFromRegistry: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { RegistrySection } from "./RegistrySection";

const OFFICIAL = { id: "aaaaaaaaaaaa", name: "official", url: "https://o.example/index.json", official: true, risk: "official" };
const EXTERNAL = { id: "bbbbbbbbbbbb", name: "mine", url: "https://x.example/index.json", official: false, risk: "external" };

function version(extra: object = {}) {
  return {
    version: "1.2.0",
    abi: "0.1",
    min_proxy: "0.1.0",
    url: "https://o.example/demo.wasm",
    sha256: "ab".repeat(32),
    size: 1234,
    signature_url: "https://o.example/demo.wasm.minisig",
    limits: { max_memory_bytes: 16777216, call_timeout_ms: 20 },
    config: "A regex matched against the first bytes.",
    ...extra,
  };
}

const view = () =>
  render(
    <MemoryRouter>
      <RegistrySection />
    </MemoryRouter>,
  );

const entry = (name: string, extra: object = {}) => ({
  name,
  description: name === "demo" ? "Detects demo traffic" : "Built for another ABI",
  license: "MIT",
  categories: ["game"],
  tags: ["demo-tag"],
  latest: "1.2.0",
  abi: "0.1",
  signed: true,
  updated: "2026-11-12",
  abi_compatible: true,
  ...extra,
});

function listing(registry = OFFICIAL) {
  return {
    registry,
    index_name: "demo registry",
    stats_url: null,
    host_abi: "0.1",
    sniffers: [entry("demo"), entry("old", { abi: "0.9", abi_compatible: false })],
  };
}

/** What opening a sniffer fetches: the full version list. */
function detail(name: string, registry = OFFICIAL, extra: object = {}) {
  return {
    registry,
    host_abi: "0.1",
    min_proxy_checked: false,
    sniffer: {
      name,
      description: entry(name).description,
      license: "MIT",
      versions: [version(name === "old" ? { abi: "0.9" } : {})],
      compatible: name === "old" ? { reason: "no version matches sniffer ABI 0.1" } : { version: "1.2.0" },
      ...extra,
    },
  };
}

function installed(extra: object = {}) {
  return {
    sniffer: "demo",
    version: "1.2.0",
    signed: false,
    risk: "official",
    sha256: "ab".repeat(32),
    results: [{ instance: "fra-1", ok: true, pinned: false, error: null, status: 200 }],
    pinned_instances: [],
    ...extra,
  };
}

beforeEach(() => {
  api.listRegistries.mockReset().mockResolvedValue({ registries: [OFFICIAL, EXTERNAL], persistent: true });
  api.listRegistrySniffers.mockReset().mockImplementation(async (id: string) =>
    listing(id === EXTERNAL.id ? EXTERNAL : OFFICIAL),
  );
  api.getRegistrySniffer.mockReset().mockImplementation(async (id: string, name: string) =>
    detail(name, id === EXTERNAL.id ? EXTERNAL : OFFICIAL),
  );
  api.addRegistry.mockReset().mockResolvedValue(EXTERNAL);
  api.removeRegistry.mockReset().mockResolvedValue(undefined);
  api.installFromRegistry.mockReset().mockResolvedValue(installed());
});

async function openInstall(name = "demo") {
  const row = (await screen.findByText(name)).closest("tr")!;
  await userEvent.click(within(row).getByRole("button", { name: "Install" }));
  return screen.getByRole("dialog");
}

describe("RegistrySection", () => {
  it("lists registries and marks external ones", async () => {
    view();
    const select = await screen.findByLabelText("Registry");
    expect(within(select).getByRole("option", { name: /official/ })).toBeInTheDocument();
    expect(within(select).getByRole("option", { name: /mine/ })).toBeInTheDocument();
    expect(await screen.findByText("official registry")).toBeInTheDocument();
    await userEvent.selectOptions(select, EXTERNAL.id);
    expect(await screen.findByText(/external, at your own risk/i)).toBeInTheDocument();
  });

  it("warns that the list is lost on restart when it is not persisted", async () => {
    api.listRegistries.mockResolvedValue({ registries: [OFFICIAL], persistent: false });
    view();
    expect(await screen.findByText(/lost on restart/i)).toBeInTheDocument();
  });

  it("adding an external registry requires confirming the risk", async () => {
    view();
    await userEvent.click(await screen.findByRole("button", { name: "Add registry" }));
    const dlg = screen.getByRole("dialog");
    await userEvent.type(within(dlg).getByLabelText("Index URL"), "https://x.example/index.json");
    const add = within(dlg).getByRole("button", { name: "Add" });
    expect(add).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(within(dlg).getByRole("checkbox"));
    expect(add).not.toHaveAttribute("aria-disabled", "true");
    await userEvent.click(add);
    expect(api.addRegistry).toHaveBeenCalledWith("https://x.example/index.json", undefined);
  });

  it("an incompatible newest version shows its ABI in the table", async () => {
    view();
    const row = (await screen.findByText("old")).closest("tr")!;
    expect(row).toHaveTextContent("built for sniffer ABI 0.9");
  });

  it("opening a sniffer fetches its versions, not the listing", async () => {
    view();
    await screen.findByText("demo");
    expect(api.getRegistrySniffer).not.toHaveBeenCalled();
    await openInstall();
    expect(api.getRegistrySniffer).toHaveBeenCalledWith(OFFICIAL.id, "demo");
  });

  it("filters the catalog by search, category and tag, and says when nothing matches", async () => {
    view();
    await screen.findByText("demo");
    await userEvent.type(screen.getByLabelText("Search"), "another");
    expect(screen.queryByText("Detects demo traffic")).toBeNull();
    expect(screen.getByText("Built for another ABI")).toBeInTheDocument();
    await userEvent.clear(screen.getByLabelText("Search"));
    await userEvent.click(within(screen.getByRole("group", { name: "Categories" })).getByRole("button", { name: /game/ }));
    expect(screen.getByText("Detects demo traffic")).toBeInTheDocument();
    await userEvent.click(within(screen.getByRole("group", { name: "Tags" })).getByRole("button", { name: /demo-tag/ }));
    await userEvent.type(screen.getByLabelText("Search"), "zzz");
    expect(screen.getByText("Nothing matches these filters.")).toBeInTheDocument();
  });

  it("offers the popular sort only when the registry reports downloads", async () => {
    view();
    await screen.findByText("demo");
    expect(screen.queryByRole("option", { name: "popular" })).toBeNull();
  });

  it("the install dialog shows limits and config", async () => {
    view();
    const dlg = await openInstall();
    expect(dlg).toHaveTextContent("1.2.0");
    expect(dlg).toHaveTextContent("16 MiB");
    expect(dlg).toHaveTextContent("20 ms");
    expect(dlg).toHaveTextContent("A regex matched against the first bytes.");
    expect(api.installFromRegistry).not.toHaveBeenCalled();
  });

  it("does not suggest a listed signature protects the install", async () => {
    view();
    const dlg = await openInstall();
    expect(dlg).toHaveTextContent("listed, not verified");
    expect(dlg).not.toHaveTextContent("listed in the registry");
  });

  it("the table only claims a signature is listed, never that the install is verified", async () => {
    view();
    const row = (await screen.findByText("demo")).closest("tr")!;
    expect(row).toHaveTextContent("signature listed");
    expect(row).not.toHaveTextContent("verified");
  });

  it("a verifiable version shows in the dialog for that version", async () => {
    api.getRegistrySniffer.mockResolvedValue(detail("demo", OFFICIAL, { versions: [version({ verifiable: true })] }));
    view();
    const dlg = await openInstall();
    expect(dlg).toHaveTextContent("listed, verified on install");
  });

  it("an external install dialog shows the risk text and needs it confirmed", async () => {
    view();
    await userEvent.selectOptions(await screen.findByLabelText("Registry"), EXTERNAL.id);
    const dlg = await openInstall();
    expect(dlg).toHaveTextContent(/at your own risk/i);
    const go = within(dlg).getByRole("button", { name: "Install on every instance" });
    expect(go).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(within(dlg).getByRole("checkbox"));
    await userEvent.click(go);
    expect(api.installFromRegistry).toHaveBeenCalledWith(EXTERNAL.id, "demo", "1.2.0");
  });

  it("an official install needs no risk confirmation", async () => {
    view();
    const dlg = await openInstall();
    expect(within(dlg).queryByRole("checkbox")).toBeNull();
    await userEvent.click(within(dlg).getByRole("button", { name: "Install on every instance" }));
    expect(api.installFromRegistry).toHaveBeenCalledWith(OFFICIAL.id, "demo", "1.2.0");
    expect(await screen.findByText(/unsigned/i)).toBeInTheDocument();
  });

  it("a partial install lists the failed instances with their reason", async () => {
    api.installFromRegistry.mockResolvedValue(
      installed({
        results: [
          { instance: "fra-1", ok: true, pinned: false, error: null, status: 200 },
          { instance: "ams-1", ok: false, pinned: false, error: "connection refused", status: null },
          {
            instance: "lon-1",
            ok: false,
            pinned: false,
            error: null,
            status: 409,
            detail: "settings.sniffers is not configured on this instance",
          },
        ],
      }),
    );
    view();
    const dlg = await openInstall();
    await userEvent.click(within(dlg).getByRole("button", { name: "Install on every instance" }));
    expect(await screen.findByText("ams-1")).toBeInTheDocument();
    expect(screen.getByText(/connection refused/)).toBeInTheDocument();
    expect(screen.getByText(/settings\.sniffers is not configured/)).toBeInTheDocument();
    expect(screen.getByText(/1 of 3 instances/)).toBeInTheDocument();
  });

  it("a pinned instance shows the pin snippet to add", async () => {
    const sha = "cd".repeat(32);
    api.installFromRegistry.mockResolvedValue(
      installed({
        results: [{ instance: "fra-1", ok: false, pinned: true, error: null, status: 409, detail: "pinned: demo" }],
        pinned_instances: [{ instance: "fra-1", pin: { name: "demo", sha256: sha } }],
      }),
    );
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    view();
    const dlg = await openInstall();
    await userEvent.click(within(dlg).getByRole("button", { name: "Install on every instance" }));
    const snippet = await screen.findByText(new RegExp(`sha256: ${sha}`));
    expect(snippet).toHaveTextContent("name: demo");
    await userEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining(`sha256: ${sha}`));
  });

  it("shows a registry error instead of an empty list", async () => {
    api.listRegistrySniffers.mockRejectedValue(new Error("registry: timed out"));
    view();
    expect(await screen.findByText(/timed out/)).toBeInTheDocument();
  });

  it("removing a registry shows it is pending and cannot be started twice", async () => {
    let finish!: () => void;
    api.removeRegistry.mockReset().mockImplementation(() => new Promise<void>((r) => (finish = r)));
    view();
    await userEvent.click(await screen.findByRole("button", { name: "remove registry" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Remove" }));
    const btn = await screen.findByRole("button", { name: "removing…" });
    expect(btn).toHaveAttribute("aria-disabled", "true");
    finish();
    expect(await screen.findByRole("button", { name: "remove registry" })).not.toHaveAttribute("aria-disabled", "true");
    expect(api.removeRegistry).toHaveBeenCalledTimes(1);
  });
});
