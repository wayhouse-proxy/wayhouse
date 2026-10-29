import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SessionProvider } from "../lib/session";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";

const api = vi.hoisted(() => ({
  listAcmeSites: vi.fn(),
  listAcmeGateways: vi.fn(),
  getAcmeAccount: vi.fn(),
  putAcmeSite: vi.fn(),
  deleteAcmeSite: vi.fn(),
  createAcmeGateway: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { CertificatesPage } from "./CertificatesPage";

const NOW = 1_800_000_000;
const DAY = 86_400;

function site(over: Record<string, unknown>) {
  return {
    name: "web",
    names: ["a.example.com"],
    gateways: [],
    challenge: "dns-01",
    dns_plugin: "p",
    enabled: true,
    challenge_zone: null,
    propagation_timeout_secs: 120,
    certificate: null,
    attempt: null,
    ...over,
  };
}

function cert(over: Record<string, unknown> = {}) {
  return {
    version: 1,
    not_after: NOW + 60 * DAY,
    issued_at: NOW - 30 * DAY,
    serial: "01",
    names: ["a.example.com"],
    key_id: "k1",
    held: null,
    untrusted: false,
    ...over,
  };
}

beforeEach(() => {
  vi.spyOn(Date, "now").mockReturnValue(NOW * 1000);
  api.getAcmeAccount.mockReset().mockResolvedValue({
    directory_url: "https://ca.test/dir",
    account_url: "https://ca.test/acct/1",
  });
  api.listAcmeGateways.mockReset().mockResolvedValue({
    gateways: [
      { name: "edge-1", created_at: NOW - 2 * DAY, sites: ["web"] },
      { name: "idle", created_at: NOW - 3600, sites: [] },
    ],
  });
  api.listAcmeSites.mockReset().mockResolvedValue({
    sites: [
      site({ name: "web", certificate: cert() }),
      site({
        name: "shop",
        names: ["shop.example.com"],
        challenge: "http-01",
        certificate: cert({ not_after: NOW + 5 * DAY }),
        attempt: { failures: 2, next_attempt_at: NOW + 3600, last_error: "CA said no" },
      }),
      site({
        name: "late",
        names: ["late.example.com"],
        certificate: cert({ not_after: NOW + 30 * DAY }),
        attempt: { failures: 1, next_attempt_at: NOW + 3600, last_error: "CA said no" },
      }),
      site({ name: "new", names: ["new.example.com"] }),
      site({ name: "gone", certificate: cert({ not_after: NOW - DAY }) }),
      site({ name: "stage", certificate: cert({ untrusted: true }) }),
      site({ name: "off", enabled: false }),
    ],
  });
});

function row(name: string) {
  // A site name can also appear in the gateways table; the sites table comes first.
  return screen.getAllByText(name, { selector: "td" })[0].closest("tr")!;
}

describe("CertificatesPage", () => {
  it("shows the CA account and one row per site with its expiry and challenge", async () => {
    render(<CertificatesPage />);
    expect(await screen.findByText("https://ca.test/acct/1")).toBeInTheDocument();
    const web = row("web");
    expect(within(web).getByText("in 60 d")).toBeInTheDocument();
    expect(within(web).getByText("dns-01")).toBeInTheDocument();
    expect(within(web).getByText("valid")).toBeInTheDocument();
    expect(within(row("shop")).getByText("http-01")).toBeInTheDocument();
  });

  it("names the state of each site", async () => {
    render(<CertificatesPage />);
    await screen.findByText("https://ca.test/acct/1");
    expect(within(row("shop")).getByText("renewal failing")).toBeInTheDocument();
    expect(within(row("new")).getByText("pending")).toBeInTheDocument();
    expect(within(row("gone")).getByText("expired")).toBeInTheDocument();
    expect(within(row("stage")).getByText("staging (untrusted)")).toBeInTheDocument();
    expect(within(row("off")).getByText("disabled")).toBeInTheDocument();
  });

  it("makes a failing renewal urgent only when the certificate is close to expiry", async () => {
    render(<CertificatesPage />);
    await screen.findByText("https://ca.test/acct/1");
    expect(within(row("late")).getByText("renewal failing").closest("[data-state]")).toHaveAttribute("data-state", "degraded");
    expect(within(row("shop")).getByText("renewal failing").closest("[data-state]")).toHaveAttribute("data-state", "down");
  });

  it("shows the last error and when the next attempt is", async () => {
    render(<CertificatesPage />);
    const shop = (await screen.findByText("shop", { selector: "td" })).closest("tr")!;
    expect(within(shop).getByText(/CA said no/)).toBeInTheDocument();
    expect(within(shop).getByText(/2 failures, next try in 1 h/)).toBeInTheDocument();
  });

  it("lists the gateways and which sites each may fetch", async () => {
    render(<CertificatesPage />);
    const edge = (await screen.findByText("edge-1")).closest("tr")!;
    expect(within(edge).getByText("web")).toBeInTheDocument();
    expect(within(screen.getByText("idle").closest("tr")!).getByText("no site")).toBeInTheDocument();
  });

  it("explains a controller without --acme", async () => {
    api.listAcmeSites.mockReset().mockRejectedValue(new ApiError(501, "ACME is off"));
    render(<CertificatesPage />);
    expect(await screen.findByText(/ACME is off/)).toBeInTheDocument();
    expect(screen.getByText("--acme")).toBeInTheDocument();
  });

  it("does not suggest --acme for a connection failure", async () => {
    api.listAcmeSites.mockReset().mockRejectedValue(new ApiError(502, "controller unreachable"));
    render(<CertificatesPage />);
    expect(await screen.findByText(/controller unreachable/)).toBeInTheDocument();
    expect(screen.queryByText("--acme")).not.toBeInTheDocument();
  });

  it("keeps the sites when the gateway or account read fails", async () => {
    api.listAcmeGateways.mockReset().mockRejectedValue(new ApiError(500, "gateway read failed"));
    api.getAcmeAccount.mockReset().mockRejectedValue(new ApiError(500, "account read failed"));
    render(<CertificatesPage />);
    expect(await screen.findByText(/Could not read gateways: gateway read failed/)).toBeInTheDocument();
    expect(screen.getByText(/Could not read CA account: account read failed/)).toBeInTheDocument();
    expect(within(row("web")).getByText("valid")).toBeInTheDocument();
  });

  it("calls an unreadable key bad even when a renewal also failed", async () => {
    api.listAcmeSites.mockReset().mockResolvedValue({
      sites: [
        site({
          name: "both",
          certificate: cert({ held: "held: key k1 missing" }),
          attempt: { failures: 1, next_attempt_at: NOW + 60, last_error: "CA said no" },
        }),
      ],
    });
    render(<CertificatesPage />);
    await screen.findByText("https://ca.test/acct/1");
    const both = row("both");
    expect(within(both).getByText("key unreadable")).toBeInTheDocument();
    expect(within(both).getByText(/CA said no/)).toBeInTheDocument();
    expect(within(both).getByText("held: key k1 missing")).toBeInTheDocument();
  });

  describe("writes", () => {
    beforeEach(() => {
      api.putAcmeSite.mockReset().mockResolvedValue(undefined);
      api.deleteAcmeSite.mockReset().mockResolvedValue(undefined);
      api.createAcmeGateway.mockReset().mockResolvedValue({ name: "edge-2", token: "tok-secret-123" });
    });

    it("adds an http-01 site with exactly the fields the controller accepts", async () => {
      render(<CertificatesPage />);
      await screen.findByText("https://ca.test/acct/1");
      await userEvent.click(screen.getByRole("button", { name: "Add site" }));
      const dlg = await screen.findByRole("dialog");
      await userEvent.type(within(dlg).getByLabelText("Site name"), "panel");
      await userEvent.type(within(dlg).getByLabelText(/^Names/), "panel.example.com");
      await userEvent.selectOptions(within(dlg).getByLabelText("Challenge"), "http-01");
      expect(within(dlg).queryByLabelText("DNS plugin")).toBeNull();
      await userEvent.click(within(dlg).getByRole("button", { name: "Add site" }));
      expect(api.putAcmeSite).toHaveBeenCalledWith("panel", { names: ["panel.example.com"], challenge: "http-01", gateways: [], enabled: true });
    });

    it("says before the call what the controller would refuse: a wildcard needs a gateway, DNS-01 needs a plugin", async () => {
      render(<CertificatesPage />);
      await screen.findByText("https://ca.test/acct/1");
      await userEvent.click(screen.getByRole("button", { name: "Add site" }));
      const dlg = await screen.findByRole("dialog");
      await userEvent.type(within(dlg).getByLabelText("Site name"), "wild");
      await userEvent.type(within(dlg).getByLabelText(/^Names/), "*.example.com");
      await userEvent.click(within(dlg).getByRole("button", { name: "Add site" }));
      expect(await within(dlg).findByText(/wildcard needs at least one gateway/)).toBeInTheDocument();
      await userEvent.click(within(dlg).getByLabelText("edge-1"));
      await userEvent.click(within(dlg).getByRole("button", { name: "Add site" }));
      expect(await within(dlg).findByText(/needs a DNS plugin/)).toBeInTheDocument();
      expect(api.putAcmeSite).not.toHaveBeenCalled();
    });

    it("shows the controller's refusal in the dialog", async () => {
      api.putAcmeSite.mockRejectedValue(new ApiError(409, "name a.example.com belongs to site web"));
      render(<CertificatesPage />);
      await screen.findByText("https://ca.test/acct/1");
      await userEvent.click(screen.getByRole("button", { name: "Edit site shop" }));
      const dlg = await screen.findByRole("dialog");
      expect(within(dlg).getByLabelText("Site name")).toBeDisabled();
      await userEvent.click(within(dlg).getByRole("button", { name: "Save site" }));
      expect(await within(dlg).findByText(/belongs to site web/)).toBeInTheDocument();
    });

    it("shows a gateway the site names but the controller no longer has, and saving can drop it", async () => {
      api.listAcmeSites.mockResolvedValue({ sites: [site({ name: "web", gateways: ["edge-1", "retired"] })] });
      render(<CertificatesPage />);
      await screen.findByText("https://ca.test/acct/1");
      await userEvent.click(screen.getByRole("button", { name: "Edit site web" }));
      const dlg = await screen.findByRole("dialog");
      const stale = within(dlg).getByLabelText(/retired/);
      expect(stale).toBeChecked();
      expect(within(dlg).getByText(/not registered/)).toBeInTheDocument();
      await userEvent.click(stale);
      await userEvent.click(within(dlg).getByRole("button", { name: "Save site" }));
      expect(api.putAcmeSite).toHaveBeenCalledWith("web", expect.objectContaining({ gateways: ["edge-1"] }));
    });

    it("removing a site asks first", async () => {
      render(<CertificatesPage />);
      await screen.findByText("https://ca.test/acct/1");
      await userEvent.click(screen.getByRole("button", { name: "Remove site off" }));
      expect(api.deleteAcmeSite).not.toHaveBeenCalled();
      const dlg = await screen.findByRole("dialog");
      await userEvent.click(within(dlg).getByRole("button", { name: "Remove" }));
      expect(api.deleteAcmeSite).toHaveBeenCalledWith("off");
    });

    it("registers a gateway and shows its token once", async () => {
      render(<CertificatesPage />);
      await screen.findByText("https://ca.test/acct/1");
      await userEvent.click(screen.getByRole("button", { name: "Register gateway" }));
      const dlg = await screen.findByRole("dialog");
      await userEvent.type(within(dlg).getByLabelText("Gateway name"), "edge-2");
      await userEvent.click(within(dlg).getByRole("button", { name: "Register" }));
      expect(await within(dlg).findByLabelText("Gateway token")).toHaveValue("tok-secret-123");
      await userEvent.keyboard("{Escape}");
      expect(screen.getByRole("dialog")).toBeInTheDocument();
      expect(within(dlg).getByRole("button", { name: "Done" })).toHaveAttribute("aria-disabled", "true");
      await userEvent.click(within(dlg).getByRole("checkbox", { name: /copied the token/ }));
      await userEvent.click(within(dlg).getByRole("button", { name: "Done" }));
      expect(screen.queryByText("tok-secret-123")).toBeNull();
      expect(screen.queryByDisplayValue("tok-secret-123")).toBeNull();
    });

    it("is read-only below admin", async () => {
      render(
        <SessionProvider value={{ username: "ana", role: "operator" }}>
          <CertificatesPage />
        </SessionProvider>,
      );
      await screen.findByText("https://ca.test/acct/1");
      expect(screen.getByRole("button", { name: "Add site" })).toHaveAttribute("aria-disabled", "true");
      expect(screen.getByRole("button", { name: "Register gateway" })).toHaveAttribute("aria-disabled", "true");
    });
  });
});
