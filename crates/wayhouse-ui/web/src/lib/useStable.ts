import { useRef } from "react";
import { sameJson } from "./sameJson";

/** The previous object while the new one has the same content, so a push that changed nothing here does not re-render what depends on it. */
export function useStable<T>(value: T): T {
  const kept = useRef(value);
  if (!sameJson(kept.current, value)) kept.current = value;
  return kept.current;
}
