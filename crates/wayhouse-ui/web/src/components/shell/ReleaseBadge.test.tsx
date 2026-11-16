import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ getRelease: vi.fn() }));
vi.mock("../../api", async (orig) => ({ ...(await orig<typeof import("../../api")>()), ...api }));
import { AboutSection } from "./AboutSection";
import { ReleaseBadge } from "./ReleaseBadge";

const status = (latest: unknown, enabled = true) => ({ current: "0.4.0", enabled, latest, checked_at: 1 });
const NEW = { version: "0.4.1", date: "2026-12-01", notes_url: "https://example.org/notes" };

function view(ui: React.ReactNode) {
  render(<QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>);
}

beforeEach(() => {
  api.getRelease.mockReset();
  localStorage.clear();
});

describe("ReleaseBadge", () => {
  it("shows a newer release with a safe link and stays gone once dismissed", async () => {
    api.getRelease.mockResolvedValue(status(NEW));
    view(<ReleaseBadge />);
    expect(await screen.findByText(/0\.4\.1 is out/)).toBeInTheDocument();
    const link = screen.getByRole("link", { name: "Release notes" });
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    await userEvent.click(screen.getByRole("button", { name: /Dismiss/ }));
    expect(screen.queryByText(/0\.4\.1 is out/)).toBeNull();
    expect(localStorage.getItem("wayhouse.release.dismissed")).toBe("0.4.1");
  });

  it("shows nothing when nothing is newer, or when that version was dismissed", async () => {
    api.getRelease.mockResolvedValue(status(null));
    view(<ReleaseBadge />);
    await vi.waitFor(() => expect(api.getRelease).toHaveBeenCalled());
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("does not render a non-https notes link", async () => {
    api.getRelease.mockResolvedValue(status({ ...NEW, notes_url: "javascript:alert(1)" }));
    view(<ReleaseBadge />);
    expect(await screen.findByText(/0\.4\.1 is out/)).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });
});

describe("AboutSection", () => {
  it("names the running and the latest version", async () => {
    api.getRelease.mockResolvedValue(status(NEW));
    view(<AboutSection />);
    expect(await screen.findByText(/Running 0\.4\.0/)).toBeInTheDocument();
    expect(screen.getByText(/Latest stable 0\.4\.1 \(2026-12-01\)/)).toBeInTheDocument();
  });

  it("says when the check is off", async () => {
    api.getRelease.mockResolvedValue(status(null, false));
    view(<AboutSection />);
    expect(await screen.findByText("The release check is off.")).toBeInTheDocument();
  });
});
