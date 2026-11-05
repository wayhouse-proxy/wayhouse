import { createContext, useContext, useEffect, useRef } from "react";
import { Outlet, useLocation } from "react-router-dom";
import { useConfigDoc, type ConfigDoc } from "./useConfigDoc";

const Ctx = createContext<ConfigDoc | null>(null);

/** The pages that edit the configuration document (General, Source and the domain editors). */
const isConfigPage = (path: string) => path === "/settings" || path === "/settings/source" || path.startsWith("/config/");

/**
 * A layout route above the whole shell: General, Source and the domain editors share one document, so a form edit
 * shows on Source and the other way round, and the unsaved changes survive leaving the settings for another page
 * and coming back. Entering a config page with nothing unsaved reads the document again (another operator may have
 * applied a revision meanwhile); closing or reloading the tab with unsaved changes asks first.
 */
export function SharedConfigDoc() {
  const doc = useConfigDoc();
  const { pathname } = useLocation();
  const onConfig = isConfigPage(pathname);
  const changes = useRef(0);
  changes.current = doc.changes;

  const first = useRef(true);
  useEffect(() => {
    if (first.current) first.current = false; // the hook's own first load covers arriving here
    else if (onConfig && changes.current === 0) void doc.reload();
  }, [onConfig]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (doc.changes === 0) return;
    const ask = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", ask);
    return () => window.removeEventListener("beforeunload", ask);
  }, [doc.changes === 0]); // eslint-disable-line react-hooks/exhaustive-deps

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
