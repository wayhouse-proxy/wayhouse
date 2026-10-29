import { useLayout } from "./useLayout";
import { useLayoutOverride } from "../uidata";

/** The layout the forms use: the shipped default with the operator's stored override on top. */
export function useFormLayout(schema: Record<string, unknown> | null) {
  const stored = useLayoutOverride();
  return useLayout(schema, stored.data);
}
