import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { deleteLayoutOverride, deleteTheme, getLayoutOverride, getUiDefaults, listThemes, saveLayoutOverride, saveTheme, saveUiDefaults, type StoredTheme, type UiDefaults } from "../api";
import { useTheme } from "./theme/provider";

/** What the controller stores for every operator of the deployment (docs/ui-data.md). A write refetches the record. */
export const useThemes = () => useQuery({ queryKey: ["ui", "themes"], queryFn: listThemes });
export const useUiDefaults = () => useQuery({ queryKey: ["ui", "defaults"], queryFn: getUiDefaults });
export const useLayoutOverride = () => useQuery({ queryKey: ["ui", "layout"], queryFn: getLayoutOverride });

function useWrite<A>(key: string, fn: (a: A) => Promise<unknown>) {
  const qc = useQueryClient();
  return useMutation({ mutationFn: fn, onSuccess: () => qc.invalidateQueries({ queryKey: ["ui", key] }) });
}
export const useSaveTheme = () => useWrite<StoredTheme>("themes", saveTheme);
export const useDeleteTheme = () => useWrite<string>("themes", deleteTheme);
export const useSaveDefaults = () => useWrite<UiDefaults>("defaults", saveUiDefaults);
export const useSaveLayout = () => useWrite<string>("layout", saveLayoutOverride);
export const useResetLayout = () => useWrite<void>("layout", deleteLayoutOverride);

/** Feeds the saved themes and the deployment default to the theme provider once the session exists. Renders nothing. */
export function ThemeSync() {
  const { setSaved } = useTheme();
  const themes = useThemes();
  const defaults = useUiDefaults();
  useEffect(() => {
    if (themes.data && !defaults.isPending) setSaved(themes.data, defaults.data?.theme);
  }, [themes.data, defaults.data, defaults.isPending, setSaved]);
  return null;
}
