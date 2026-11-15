import { Component, Suspense, useEffect, useState } from "react";
import { Outlet, useLocation } from "react-router-dom";
import { Menu } from "lucide-react";
import { Button } from "../ui/Button";
import { InspectorPanel, InspectorProvider } from "./Inspector";
import { breadcrumbFor, NAV, type NavGroup } from "./nav";
import { Rail } from "./Rail";
import { ErrorState, Loading } from "./States";

/**
 * A page that cannot be loaded: after an upgrade an open tab asks for a hashed chunk the new build no longer
 * serves, and the lazy import rejects. Without this the whole app unmounts. Keyed by path, so another page clears it.
 */
class PageBoundary extends Component<{ children: React.ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  render() {
    if (!this.state.error) return this.props.children;
    return (
      <ErrorState>
        This page could not be loaded, most likely because the console was upgraded since this tab opened. Reload to get the new version.
        <Button size="sm" className="mt-2 block" onClick={() => window.location.reload()}>
          Reload
        </Button>
      </ErrorState>
    );
  }
}

/**
 * Three zones: the rail on the left, the page in the middle, the inspector on the
 * right (below the page under 1100 px). Under 760 px the rail becomes a top bar with a menu.
 */
export function Shell({ onSignOut, nav = NAV, banner }: { onSignOut: () => void; nav?: NavGroup[]; banner?: React.ReactNode }) {
  const { pathname } = useLocation();
  const [menu, setMenu] = useState(false);
  // Picking a page on a narrow screen must reveal it, not leave the rail on top.
  useEffect(() => setMenu(false), [pathname]);
  const crumbs = breadcrumbFor(nav, pathname);

  return (
    <InspectorProvider>
      <div className="flex min-h-screen flex-col min-[760px]:h-screen min-[760px]:flex-row">
        <div className="flex items-center justify-between border-b border-line bg-rail px-3 py-2 text-rail-ink min-[760px]:hidden" data-rail>
          <span className="text-sm font-semibold">wayhouse</span>
          <Button variant="ghost" size="sm" aria-expanded={menu} aria-controls="rail" onClick={() => setMenu(!menu)} className="text-rail-ink hover:bg-rail-hover">
            <Menu aria-hidden size={16} strokeWidth={1.5} /> Menu
          </Button>
        </div>
        <div id="rail" className={`${menu ? "block" : "hidden"} w-full shrink-0 border-b border-line min-[760px]:block min-[760px]:border-b-0 min-[760px]:w-(--rail-width)`}>
          <Rail nav={nav} onSignOut={onSignOut} />
        </div>
        <div className="flex min-w-0 flex-1 flex-col min-[1100px]:flex-row min-[760px]:overflow-hidden">
          <main className="min-w-0 flex-1 overflow-y-auto p-(--page-pad)">
            {banner}
            {crumbs.length > 0 && (
              <nav aria-label="Breadcrumb" className="mb-1 text-xs text-ink-muted">
                {crumbs.join(" / ")}
              </nav>
            )}
            <PageBoundary key={pathname}>
              <Suspense fallback={<Loading />}>
                <Outlet />
              </Suspense>
            </PageBoundary>
          </main>
          <InspectorPanel />
        </div>
      </div>
    </InspectorProvider>
  );
}
