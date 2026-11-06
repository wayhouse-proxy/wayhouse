import type { PluginInstall } from "../../types";

/** The `created_by` the controller gives installs its plugins file owns (docs/plugins.md). */
export const FILE_ACTOR = "plugins-file";

/** An install the plugins file owns is changed in the file; the API answers 409 `managed_by_file`. */
export const managedByFile = (p: Pick<PluginInstall, "created_by">) => p.created_by === FILE_ACTOR;
