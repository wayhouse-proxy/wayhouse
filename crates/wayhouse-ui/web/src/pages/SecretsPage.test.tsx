import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import { SessionProvider, type Role } from "../lib/session";

const api = vi.hoisted(() => ({
  listClusterSecrets: vi.fn(),
  putClusterSecret: vi.fn(),
  deleteClusterSecret: vi.fn(),
  getPluginSecretKeys: vi.fn(),
  rewrapPluginSecrets: vi.fn(),
  rewrapClusterSecrets: vi.fn(),
  rewrapAcmeKeys: vi.fn(),
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

  it("re-encrypts the cluster secrets and the certificate keys with their own buttons", async () => {
    api.getPluginSecretKeys.mockResolvedValue({ key_ids: ["k1", "k2"], active: "k2", nodes: {} });
    api.rewrapClusterSecrets.mockResolvedValue({ rewrapped: 1, skipped: ["gossip-psk"], unreadable: [], active_key_id: "k2" });
    api.rewrapAcmeKeys.mockResolvedValue({ rewrapped: 2, skipped: [], unreadable: [], active_key_id: "k2" });
    const user = userEvent.setup();
    page();
    await screen.findByRole("table", { name: "Keys per node" });
    await user.click(screen.getByRole("button", { name: "Re-encrypt cluster secrets" }));
    expect(api.rewrapClusterSecrets).toHaveBeenCalledTimes(1);
    expect(await screen.findByText(/Re-encrypted 1 cluster secret under k2\. 1 could not be re-encrypted: gossip-psk/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Re-encrypt certificate keys" }));
    expect(await screen.findByText(/Re-encrypted 2 certificate keys under k2/)).toBeInTheDocument();
    expect(api.rewrapPluginSecrets).not.toHaveBeenCalled();
  });

  it("says so when the controller runs without ACME", async () => {
    api.getPluginSecretKeys.mockResolvedValue({ key_ids: ["k1", "k2"], active: "k2", nodes: {} });
    api.rewrapAcmeKeys.mockRejectedValue(new ApiError(404, "not found"));
    const user = userEvent.setup();
    page();
    await screen.findByRole("table", { name: "Keys per node" });
    await user.click(screen.getByRole("button", { name: "Re-encrypt certificate keys" }));
    expect(await screen.findByText(/no certificate keys to re-encrypt/)).toBeInTheDocument();
  });

  it("will not re-encrypt while a node lacks the active key", async () => {
    page();
    await screen.findByRole("table", { name: "Keys per node" });
    for (const name of ["Re-encrypt plugin secrets", "Re-encrypt cluster secrets", "Re-encrypt certificate keys"]) {
      expect(screen.getByRole("button", { name })).toHaveAttribute("aria-disabled", "true");
    }
  });

  it("shows no key rotation when the controller has no plugins (404)", async () => {
    api.getPluginSecretKeys.mockRejectedValue(new ApiError(404, "not found"));
    page();
    await screen.findByText("gossip-psk");
    await vi.waitFor(() => expect(api.getPluginSecretKeys).toHaveBeenCalled());
    expect(screen.queryByRole("region", { name: "Key rotation" })).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("an operator neither sees the key rotation nor triggers the admin-only key request", async () => {
    page("operator");
    await screen.findByText("gossip-psk");
    expect(screen.queryByRole("region", { name: "Key rotation" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Re-encrypt plugin secrets" })).toBeNull();
    expect(api.getPluginSecretKeys).not.toHaveBeenCalled();
  });
});
