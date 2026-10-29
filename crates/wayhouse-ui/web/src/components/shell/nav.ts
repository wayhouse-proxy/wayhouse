import type { Role } from "../../lib/session";

export interface NavItem {
  to: string;
  label: string;
  /** Hidden from sessions below this role; defaults to viewer. */
  minRole?: Role;
  /** A second level that expands in place under its parent. */
  children?: NavItem[];
}

export interface NavGroup {
  label: string;
  items: NavItem[];
}

export const NAV: NavGroup[] = [
  {
    label: "Operate",
    items: [
      { to: "/dashboard", label: "Dashboard" },
      { to: "/overview", label: "Overview" },
      { to: "/fleet", label: "Fleet" },
      { to: "/listeners", label: "Listeners" },
      { to: "/pools", label: "Pools" },
    ],
  },
  {
    label: "Configure",
    items: [
      {
        to: "/settings",
        label: "Settings",
        children: [
          { to: "/settings", label: "General" },
          { to: "/settings/source", label: "Source" },
          { to: "/settings/appearance", label: "Appearance" },
          { to: "/settings/layout", label: "Layout" },
        ],
      },
      { to: "/config/listeners", label: "Listeners and routes" },
      { to: "/config/pools", label: "Pools" },
      { to: "/config/sources", label: "Backend sources" },
      { to: "/config/resolvers", label: "Resolvers" },
      { to: "/config/web", label: "Web sites" },
      { to: "/sniffers", label: "Sniffers" },
      { to: "/plugins", label: "Plugins" },
      { to: "/certificates", label: "Certificates" },
    ],
  },
  {
    label: "System",
    items: [
      { to: "/config-history", label: "Config history" },
      { to: "/tunnel", label: "Tunnel addresses" },
    ],
  },
];

/** "Configure / Settings" for the current path, or [] when it matches no item. */
export function breadcrumbFor(nav: NavGroup[], path: string): string[] {
  for (const g of nav) {
    for (const item of g.items) {
      const sub = item.children?.find((c) => matches(c.to, path));
      if (sub) return [g.label, item.label, sub.label];
      if (matches(item.to, path)) return [g.label, item.label];
    }
  }
  return [];
}

export const matches = (to: string, path: string) => path === to || path.startsWith(`${to}/`);
