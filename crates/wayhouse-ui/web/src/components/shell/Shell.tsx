import { useState } from "react";
import { Outlet, useLocation } from "react-router-dom";
import { Menu } from "lucide-react";
import { Button } from "../ui/Button";
import { InspectorPanel, InspectorProvider } from "./Inspector";
import { breadcrumbFor, NAV, type NavGroup } from "./nav";
import { Rail } from "./Rail";

/**
 * Three zones: the dark rail on the left, the page in the middle, the inspector on the
 * right (below the page under 1100 px). Under 760 px the rail becomes a top bar with a menu.
 */
export function Shell({ onSignOut, nav = NAV }: { onSignOut: () => void; nav?: NavGroup[] }) {
  const { pathname } = useLocation();
  const [menu, setMenu] = useState(false);
  const crumbs = breadcrumbFor(nav, pathname);

  return (
    <InspectorProvider>
      <div className="flex min-h-screen flex-col min-[760px]:h-screen min-[760px]:flex-row">
        <div className="flex items-center justify-between bg-rail px-3 py-2 text-rail-ink min-[760px]:hidden" data-rail>
          <span className="text-sm font-semibold">wayhouse</span>
          <Button variant="ghost" size="sm" aria-expanded={menu} aria-controls="rail" onClick={() => setMenu(!menu)} className="text-rail-ink hover:bg-rail-hover">
            <Menu aria-hidden size={16} strokeWidth={1.5} /> Menu
          </Button>
        </div>
        <div id="rail" className={`${menu ? "block" : "hidden"} w-full shrink-0 min-[760px]:block min-[760px]:w-(--rail-width)`}>
          <Rail nav={nav} onSignOut={onSignOut} />
        </div>
        <div className="flex min-w-0 flex-1 flex-col min-[1100px]:flex-row min-[760px]:overflow-hidden">
          <main className="min-w-0 flex-1 overflow-y-auto p-(--page-pad)">
            {crumbs.length > 0 && (
              <nav aria-label="Breadcrumb" className="mb-1 text-xs text-ink-muted">
                {crumbs.join(" / ")}
              </nav>
            )}
            <Outlet />
          </main>
          <InspectorPanel />
        </div>
      </div>
    </InspectorProvider>
  );
}
