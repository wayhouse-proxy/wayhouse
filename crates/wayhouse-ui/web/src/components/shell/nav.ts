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
  { label: "Operate", items: [{ to: "/fleet", label: "Fleet" }] },
  {
    label: "Configure",
    items: [
      { to: "/settings", label: "Settings" },
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
