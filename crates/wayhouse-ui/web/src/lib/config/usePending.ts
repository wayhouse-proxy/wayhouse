import { useQuery } from "@tanstack/react-query";
import { getPendingConfig } from "../../api";

export const PENDING_KEY = ["config", "pending"] as const;

/** The revision on trial and what counts as risky. The controller owns the clock; this only re-reads it. */
export const usePending = () =>
  useQuery({ queryKey: PENDING_KEY, queryFn: getPendingConfig, refetchInterval: 5000, retry: false });
