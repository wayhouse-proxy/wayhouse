import { createContext, useContext } from "react";

export type Role = "viewer" | "operator" | "admin";
const ORDER: Role[] = ["viewer", "operator", "admin"];

export interface Session {
  username: string | null;
  role: Role;
}

const SessionCtx = createContext<Session>({ username: null, role: "admin" });

export const SessionProvider = SessionCtx.Provider;
export const useSession = () => useContext(SessionCtx);

/** Whether the session's role is at least `min`. The server gates stay the authority; this only hides what would be refused. */
export const atLeast = (role: Role, min: Role) => ORDER.indexOf(role) >= ORDER.indexOf(min);
export const useCan = (min: Role) => atLeast(useSession().role, min);
