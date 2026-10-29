import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ getPendingConfig: vi.fn(), confirmRevision: vi.fn(), revertNow: vi.fn() }));
vi.mock("../../api", async (orig) => ({ ...(await orig<typeof import("../../api")>()), ...api }));
import { SessionProvider } from "../../lib/session";
import { TrialBar } from "./TrialBar";

const RISKS = [{ id: "admin_listen", text: "settings.admin.listen changed" }];
const pending = (remaining_ms: number) => ({
  pending: { revision: 9, revert_to: 8, deadline_ms: 1000 + remaining_ms, reasons: ["admin_listen"], remaining_ms },
  now_ms: 1000,
  auto_confirm_secs: 60,
  risks: RISKS,
});

function bar() {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <TrialBar />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  api.confirmRevision.mockReset().mockResolvedValue(undefined);
  api.revertNow.mockReset().mockResolvedValue(undefined);
});
afterEach(() => vi.useRealTimers());

describe("TrialBar", () => {
  it("shows the trial to a viewer but leaves both buttons off", async () => {
    api.getPendingConfig.mockResolvedValue(pending(45_000));
    render(
      <SessionProvider value={{ username: "ana", role: "viewer" }}>
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <TrialBar />
        </QueryClientProvider>
      </SessionProvider>,
    );
    await screen.findByText(/Applied as revision 9/);
    expect(screen.getByRole("button", { name: "Keep changes" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "Revert now" })).toHaveAttribute("aria-disabled", "true");
  });

  it("renders nothing when no revision is on trial", async () => {
    api.getPendingConfig.mockResolvedValue({ pending: null, now_ms: 1, auto_confirm_secs: 60, risks: RISKS });
    bar();
    await act(async () => {});
    expect(screen.queryByRole("button", { name: "Keep changes" })).not.toBeInTheDocument();
  });

  it("shows the controller's countdown, the reason and both ways out", async () => {
    api.getPendingConfig.mockResolvedValue(pending(45_000));
    bar();
    expect(await screen.findByText(/Applied as revision 9/)).toHaveTextContent("reverts to revision 8");
    expect(screen.getByText(/45 s/)).toBeInTheDocument();
    expect(screen.getByText(/settings.admin.listen changed/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Keep changes" }));
    expect(api.confirmRevision).toHaveBeenCalledWith(9);
    await userEvent.click(screen.getByRole("button", { name: "Revert now" }));
    expect(api.revertNow).toHaveBeenCalledWith(9);
  });

  it("counts down locally and announces only at 30, 10 and 0 seconds", async () => {
    // Like the controller: every answer carries what is left now.
    const start = Date.now();
    api.getPendingConfig.mockImplementation(async () => pending(Math.max(0, 32_000 - (Date.now() - start))));
    bar();
    await screen.findByText(/Applied as revision 9/);
    const live = () => screen.getByRole("status").textContent;
    expect(live()).toMatch(/Applied on trial/);
    await act(async () => void vi.advanceTimersByTime(3000));
    expect(live()).toBe("30 seconds left to confirm.");
    for (let i = 0; i < 22; i++) await act(async () => void vi.advanceTimersByTime(1000));
    expect(live()).toBe("10 seconds left to confirm.");
    for (let i = 0; i < 8; i++) await act(async () => void vi.advanceTimersByTime(1000));
    expect(live()).toMatch(/Time is up/);
  });
});
