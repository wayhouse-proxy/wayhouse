import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { Shell } from "./Shell";
import { RequireRole } from "./Guard";
import { breadcrumbFor, type NavGroup } from "./nav";
import { useInspector } from "./Inspector";
import { SessionProvider, type Role } from "../../lib/session";

const NAV: NavGroup[] = [
  { label: "Operate", items: [{ to: "/fleet", label: "Fleet" }] },
  {
    label: "Configure",
    items: [
      { to: "/settings", label: "Settings", children: [{ to: "/settings/network", label: "Network" }, { to: "/settings/secrets", label: "Secrets", minRole: "admin" }] },
      { to: "/plugins", label: "Plugins", minRole: "operator" },
    ],
  },
];

function Opener() {
  const { open } = useInspector();
  return <button onClick={() => open("ams-edge-01", <p>details</p>)}>row</button>;
}

function setup(role: Role, path = "/fleet") {
  return render(
    <SessionProvider value={{ username: "ana", role }}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route element={<Shell nav={NAV} onSignOut={() => {}} />}>
            <Route path="/fleet" element={<Opener />} />
            <Route path="/settings/*" element={<p>settings page</p>} />
            <Route path="/plugins" element={<RequireRole min="operator"><p>plugins page</p></RequireRole>} />
          </Route>
        </Routes>
      </MemoryRouter>
    </SessionProvider>,
  );
}

describe("navigation", () => {
  it("groups the items and marks the current page", () => {
    setup("admin");
    const rail = screen.getByRole("navigation", { name: "Main" });
    expect(within(rail).getByRole("heading", { name: "Operate" })).toBeInTheDocument();
    expect(within(rail).getByRole("link", { name: "Fleet" })).toHaveAttribute("aria-current", "page");
  });

  it("hides items above the session's role and the second level too", () => {
    setup("viewer");
    const rail = screen.getByRole("navigation", { name: "Main" });
    expect(within(rail).queryByRole("link", { name: "Plugins" })).not.toBeInTheDocument();
    expect(within(rail).getByRole("button", { name: "Settings" })).toBeInTheDocument();
  });

  it("a parent with children toggles in place and shows only the children the role may use", async () => {
    setup("operator");
    const parent = screen.getByRole("button", { name: "Settings" });
    expect(parent).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(parent);
    expect(parent).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "Network" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Secrets" })).not.toBeInTheDocument();
  });

  it("builds the breadcrumb from the nav", () => {
    expect(breadcrumbFor(NAV, "/settings/network")).toEqual(["Configure", "Settings", "Network"]);
    expect(breadcrumbFor(NAV, "/fleet")).toEqual(["Operate", "Fleet"]);
    expect(breadcrumbFor(NAV, "/nowhere")).toEqual([]);
  });

  it("shows a breadcrumb above the page", () => {
    setup("admin", "/settings/network");
    expect(screen.getByRole("navigation", { name: "Breadcrumb" })).toHaveTextContent("Configure / Settings / Network");
  });
});

describe("role gating of a page", () => {
  it("shows the Forbidden state to a viewer and the page to an operator", () => {
    setup("viewer", "/plugins");
    expect(screen.getByRole("alert")).toHaveTextContent("Not allowed");
    expect(screen.queryByText("plugins page")).not.toBeInTheDocument();
  });

  it("lets an operator through", () => {
    setup("operator", "/plugins");
    expect(screen.getByText("plugins page")).toBeInTheDocument();
  });
});

describe("narrow screens", () => {
  it("has a menu button that toggles the rail", async () => {
    setup("admin");
    const menu = screen.getByRole("button", { name: /Menu/ });
    expect(menu).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(menu);
    expect(menu).toHaveAttribute("aria-expanded", "true");
  });
});

describe("inspector", () => {
  it("focuses its heading on open, closes with Escape and returns focus to the row", async () => {
    setup("admin");
    const row = screen.getByRole("button", { name: "row" });
    await userEvent.click(row);
    const heading = screen.getByRole("heading", { name: "ams-edge-01" });
    expect(heading).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("complementary", { name: "Details" })).not.toBeInTheDocument();
    expect(row).toHaveFocus();
  });
});
