import { useMemo } from "react";
import { DEFAULT_LAYOUT, knownPaths, mergeLayout, parseOverride, type Layout } from "./layout";
import { simplify } from "./schema";

/**
 * The layout the forms use: the shipped default with the operator's override on top. A missing override means the
 * default; an unusable one (not JSON, wrong shape) also means the default, with `notice` saying why, and never stops editing.
 * `override` is the stored text (null when none is stored); fetching it from the controller is wired where B3's endpoint lands.
 */
export function useLayout(schema: Record<string, unknown> | null, override: string | null | undefined): { layout: Layout; notice: string | null } {
  return useMemo(() => {
    if (!override?.trim() || !schema) return { layout: DEFAULT_LAYOUT, notice: null };
    const r = parseOverride(override, knownPaths(simplify(schema)));
    if (r.error) return { layout: DEFAULT_LAYOUT, notice: `The form layout override is not used: ${r.error}` };
    return { layout: mergeLayout(DEFAULT_LAYOUT, r.layout), notice: r.warnings.length ? `The form layout override has problems: ${r.warnings.join(" ")}` : null };
  }, [schema, override]);
}
