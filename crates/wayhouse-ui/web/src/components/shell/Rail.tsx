import { useState } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { ChevronRight } from "lucide-react";
import { cn } from "../../lib/cn";
import { atLeast, useSession } from "../../lib/session";
import { matches, type NavGroup, type NavItem } from "./nav";

const row =
  "flex min-h-8 w-full items-center gap-1.5 rounded-sm px-2 text-left text-sm text-rail-ink hover:bg-rail-hover";
const current = "bg-rail-hover font-semibold";

function Item({ item }: { item: NavItem }) {
  const { pathname } = useLocation();
  const open = item.children?.some((c) => matches(c.to, pathname)) || matches(item.to, pathname);
  const [expanded, setExpanded] = useState(open);
  const { role } = useSession();
  const children = item.children?.filter((c) => atLeast(role, c.minRole ?? "viewer"));

  if (!children?.length)
    return (
      <NavLink to={item.to} className={({ isActive }) => cn(row, isActive && current)}>
        {item.label}
      </NavLink>
    );
  return (
    <div>
      <button type="button" aria-expanded={expanded} onClick={() => setExpanded(!expanded)} className={cn(row, open && !expanded && current)}>
        <ChevronRight
          aria-hidden
          size={16}
          strokeWidth={1.5}
          className={cn("transition-transform duration-(--dur-fast)", expanded && "rotate-90")}
        />
        {item.label}
      </button>
      {expanded && (
        <div className="ml-4 flex flex-col gap-0.5">
          {children.map((c) => (
            <NavLink
              key={c.to}
              to={c.to}
              className={({ isActive }) => cn(row, "text-rail-ink-muted", isActive && cn(current, "text-rail-ink"))}
            >
              {c.label}
            </NavLink>
          ))}
        </div>
      )}
    </div>
  );
}

/** The dark navigation rail: brand, groups, items (with an optional second level), sign out. */
export function Rail({ nav, onSignOut }: { nav: NavGroup[]; onSignOut: () => void }) {
  const { role } = useSession();
  return (
    <nav aria-label="Main" data-rail className="flex h-full flex-col bg-rail px-2 py-3 text-rail-ink">
      <div className="flex items-center gap-2 px-2 pb-4">
        <span aria-hidden className="h-2 w-2 bg-rail-ink" />
        <span className="text-sm font-semibold">wayhouse</span>
      </div>
      <div className="flex flex-1 flex-col gap-4 overflow-y-auto">
        {nav.map((g) => {
          const items = g.items.filter((i) => atLeast(role, i.minRole ?? "viewer"));
          if (!items.length) return null;
          return (
            <div key={g.label}>
              <h2 className="px-2 pb-1 text-xs font-semibold tracking-wider text-rail-ink-muted uppercase">{g.label}</h2>
              <div className="flex flex-col gap-0.5">
                {items.map((i) => (
                  <Item key={i.to} item={i} />
                ))}
              </div>
            </div>
          );
        })}
      </div>
      <button type="button" onClick={onSignOut} className={cn(row, "text-rail-ink-muted")}>
        Sign out
      </button>
    </nav>
  );
}
