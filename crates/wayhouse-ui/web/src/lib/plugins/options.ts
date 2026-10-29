import type { PluginManifest } from "../../types";
import type { Layout } from "../form/layout";

/** Everything the options form needs from a manifest: the schema minus secrets, the layout from its hints. */
export function optionsForm(manifest: PluginManifest | null | undefined) {
  const options = manifest?.options ?? {};
  const secrets: string[] = [];
  const properties: Record<string, any> = {};
  const fields: Layout["fields"] = {};
  for (const [name, p] of Object.entries<any>(options.properties ?? {})) {
    if (p["x-wayhouse-secret"]) {
      secrets.push(name);
      continue;
    }
    properties[name] = p;
    if (p.group || p.order !== undefined) fields[name] = { group: p.group, order: p.order };
  }
  const schema = {
    ...options,
    type: "object",
    properties,
    required: (options.required ?? []).filter((n: string) => n in properties),
  };
  return { schema, layout: { version: 1, fields } as Layout, secrets, hasFields: Object.keys(properties).length > 0 };
}
