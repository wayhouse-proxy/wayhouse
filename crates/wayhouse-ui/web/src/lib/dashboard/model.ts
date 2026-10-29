import { requestJson } from "../../api";

/** One widget on a dashboard: a stable versioned id, its width in grid columns (1 to 12) and optional settings. */
export interface PlacedWidget {
  id: string;
  width: number;
  settings?: Record<string, unknown>;
}
/** The stored and exported shape (docs/ui-data.md, components.md): `{ name, widgets: [{ id, width }] }`. */
export interface Dashboard {
  name: string;
  widgets: PlacedWidget[];
}

export const MAX_WIDGETS = 100;
export const WIDTHS: { value: number; label: string }[] = [
  { value: 4, label: "Small (4 columns)" },
  { value: 6, label: "Medium (6 columns)" },
  { value: 12, label: "Wide (12 columns)" },
];

/** What a first-time visitor sees until somebody saves a dashboard. It is not stored. */
export const DEFAULT_DASHBOARD: Dashboard = {
  name: "Overview",
  widgets: [
    { id: "nodes.v1", width: 6 },
    { id: "attention.v1", width: 6 },
    { id: "connections.v1", width: 6 },
    { id: "throughput.v1", width: 6 },
    { id: "rollout.v1", width: 6 },
    { id: "certificates.v1", width: 6 },
  ],
};

/** Reads an imported dashboard. Throws a sentence naming what is wrong; an unknown widget id is kept (it shows a placeholder). */
export function parseDashboard(text: string): Dashboard {
  let raw: any;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    throw new Error(`Not valid JSON: ${(e as Error).message}`);
  }
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) throw new Error("A dashboard is a JSON object with name and widgets.");
  if (!Array.isArray(raw.widgets)) throw new Error('"widgets" must be a list.');
  if (raw.widgets.length > MAX_WIDGETS) throw new Error(`A dashboard has at most ${MAX_WIDGETS} widgets.`);
  const widgets = raw.widgets.map((w: any, i: number): PlacedWidget => {
    if (typeof w?.id !== "string" || !w.id) throw new Error(`widgets[${i}].id must be a text.`);
    if (!Number.isInteger(w.width) || w.width < 1 || w.width > 12) throw new Error(`widgets[${i}].width must be an integer from 1 to 12.`);
    const out: PlacedWidget = { id: w.id, width: w.width };
    if (w.settings && typeof w.settings === "object" && !Array.isArray(w.settings)) out.settings = w.settings;
    return out;
  });
  return { name: typeof raw.name === "string" ? raw.name : "", widgets };
}

export const exportDashboard = (d: Dashboard) => JSON.stringify({ name: d.name, widgets: d.widgets }, null, 2);

/** The controller's rule for a name (docs/ui-data.md): 1 to 64 characters, no `/` or control characters. */
export function dashboardNameProblem(name: string, taken: string[] = []): string | null {
  if (!name.trim()) return "Give the dashboard a name.";
  if (name.length > 64) return "A name has at most 64 characters.";
  // eslint-disable-next-line no-control-regex
  if (name.includes("/") || /[\u0000-\u001f\u007f]/.test(name)) return "A name cannot contain / or control characters.";
  if (taken.includes(name)) return `A dashboard named "${name}" already exists.`;
  return null;
}

/** Moves the widget at `i` by `by` places (clamped). Returns the same list when nothing moves. */
export function move<T>(list: T[], i: number, by: number): T[] {
  const j = Math.max(0, Math.min(list.length - 1, i + by));
  if (j === i) return list;
  const next = [...list];
  next.splice(j, 0, next.splice(i, 1)[0]);
  return next;
}

export async function listDashboards(): Promise<Dashboard[]> {
  return (await requestJson<{ items?: Dashboard[] }>("/api/ui/dashboards")).items ?? [];
}
export function saveDashboard(d: Dashboard): Promise<Dashboard> {
  return requestJson(`/api/ui/dashboards/${encodeURIComponent(d.name)}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(d),
  });
}
export function deleteDashboard(name: string): Promise<void> {
  return requestJson(`/api/ui/dashboards/${encodeURIComponent(name)}`, { method: "DELETE" });
}
