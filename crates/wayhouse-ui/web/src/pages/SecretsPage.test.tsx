import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SessionProvider, type Role } from "../lib/session";

const api = vi.hoisted(() => ({
  listClusterSecrets: vi.fn(),
  putClusterSecret: vi.fn(),
  deleteClusterSecret: vi.fn(),
  getPluginSecretKeys: vi.fn(),
  rewrapPluginSecrets: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { SecretsPage } from "./SecretsPage";

const page = (role: Role = "admin") =>
  render(
    <SessionProvider value={{ username: "ana", role }}>
      <SecretsPage />
    </SessionProvider>,
  );

beforeEach(() => {
  Object.values(api).forEach((f) => f.mockReset());
  api.listClusterSecrets.mockResolvedValue([{ name: "gossip-psk", key_id: "k1", set_at: 1_800_000_000, actor: "ana" }]);
  api.putClusterSecret.mockResolvedValue(undefined);
  api.deleteClusterSecret.mockResolvedValue(undefined);
  api.getPluginSecretKeys.mockResolvedValue({ key_ids: ["k1", "k2"], active: "k2", nodes: { "1": ["k1", "k2"], "2": ["k1"], "3": { error: "no answer" } } });
  api.rewrapPluginSecrets.mockResolvedValue({ rewrapped: 3, skipped: [], unreadable: [], active_key_id: "k2" });
});

describe("SecretsPage", () => {
  it("lists names and key ids, never a value", async () => {
    page();
    expect(await screen.findByText("gossip-psk")).toBeInTheDocument();
    expect(screen.getAllByText("k1")[0]).toBeInTheDocument();
    expect(screen.getByText(/cannot be read back/i)).toBeInTheDocument();
  });

  it("sends a new secret once and clears the field", async () => {
    const user = userEvent.setup();
    page();
    await screen.findByText("gossip-psk");
    await user.click(screen.getByRole("button", { name: "Add secret" }));
    await user.type(screen.getByLabelText("Name"), "admin-token");
    const value = screen.getByLabelText("Value");
    expect(value).toHaveAttribute("type", "password");
    await user.type(value, "s3cret-value");
    await user.click(screen.getByRole("button", { name: "Save secret" }));
    expect(api.putClusterSecret).toHaveBeenCalledWith("admin-token", "s3cret-value");
    expect(api.listClusterSecrets).toHaveBeenCalledTimes(2);
    expect(screen.queryByDisplayValue("s3cret-value")).toBeNull();
  });

  it("says a name is taken, and saving then replaces", async () => {
    const user = userEvent.setup();
    page();
    await screen.findByText("gossip-psk");
    await user.click(screen.getByRole("button", { name: "Add secret" }));
    await user.type(screen.getByLabelText("Name"), "gossip-psk");
    expect(screen.getByText(/gossip-psk.* exists/)).toBeInTheDocument();
    await user.type(screen.getByLabelText("Value"), "s3cret-value");
    await user.click(screen.getByRole("button", { name: "Replace secret" }));
    expect(api.putClusterSecret).toHaveBeenCalledWith("gossip-psk", "s3cret-value");
  });

  it("refuses a name the controller would refuse, before sending", async () => {
    const user = userEvent.setup();
    page();
    await screen.findByText("gossip-psk");
    await user.click(screen.getByRole("button", { name: "Add secret" }));
    await user.type(screen.getByLabelText("Name"), "Bad_Name");
    await user.type(screen.getByLabelText("Value"), "s3cret-value");
    expect(screen.getByRole("button", { name: "Save secret" })).toHaveAttribute("aria-disabled", "true");
    expect(api.putClusterSecret).not.toHaveBeenCalled();
  });

  it("deletes after naming the secret in the button", async () => {
    const user = userEvent.setup();
    page();
    await screen.findByText("gossip-psk");
    await user.click(screen.getByRole("button", { name: "Delete gossip-psk" }));
    await user.click(screen.getByRole("button", { name: "Yes, delete" }));
    expect(api.deleteClusterSecret).toHaveBeenCalledWith("gossip-psk");
  });

  it("gives a viewer no way to write", async () => {
    page("viewer");
    await screen.findByText("gossip-psk");
    expect(screen.getByRole("button", { name: "Add secret" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "Delete gossip-psk" })).toHaveAttribute("aria-disabled", "true");
  });

  it("shows the key ids per controller node and flags a node that lacks the active key", async () => {
    page();
    const table = await screen.findByRole("table", { name: "Keys per node" });
    expect(table).toHaveTextContent("k2 (active)");
    expect(screen.getByText(/node 2 does not hold the active key k2/i)).toBeInTheDocument();
    expect(screen.getByText(/no answer/)).toBeInTheDocument();
  });

  it("re-encrypts the plugin secrets under the active key and reports the count", async () => {
    api.getPluginSecretKeys.mockResolvedValue({ key_ids: ["k1", "k2"], active: "k2", nodes: {} });
    const user = userEvent.setup();
    page();
    await screen.findByRole("table", { name: "Keys per node" });
    await user.click(screen.getByRole("button", { name: "Re-encrypt plugin secrets" }));
    expect(api.rewrapPluginSecrets).toHaveBeenCalledTimes(1);
    expect(await screen.findByText(/Re-encrypted 3 plugin secrets under k2/)).toBeInTheDocument();
  });

  it("will not re-encrypt while a node lacks the active key", async () => {
    page();
    await screen.findByRole("table", { name: "Keys per node" });
    expect(screen.getByRole("button", { name: "Re-encrypt plugin secrets" })).toHaveAttribute("aria-disabled", "true");
  });

  it("an operator cannot re-encrypt", async () => {
    page("operator");
    await screen.findByText("gossip-psk");
    expect(screen.queryByRole("button", { name: "Re-encrypt plugin secrets" })).toBeNull();
  });
});
