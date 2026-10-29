import { createContext, useContext } from "react";
import { Outlet } from "react-router-dom";
import { useConfigDoc, type ConfigDoc } from "./useConfigDoc";

const Ctx = createContext<ConfigDoc | null>(null);

/**
 * A layout route for the pages that edit the configuration: General, Source and the domain editors share one
 * document, so a form edit shows on Source and the other way round, and the unsaved changes survive moving between them.
 */
export function SharedConfigDoc() {
  const doc = useConfigDoc();
  return (
    <Ctx.Provider value={doc}>
      <Outlet />
    </Ctx.Provider>
  );
}

/** The shared document; a page outside the layout route is a bug, not a fallback. */
export function useSharedConfigDoc(): ConfigDoc {
  const doc = useContext(Ctx);
  if (!doc) throw new Error("useSharedConfigDoc needs the SharedConfigDoc layout route");
  return doc;
}
