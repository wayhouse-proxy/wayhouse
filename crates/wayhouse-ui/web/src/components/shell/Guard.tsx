import { atLeast, useSession, type Role } from "../../lib/session";
import { Forbidden } from "./States";

/** Shows its children only to a session whose role is at least `min`; otherwise the Forbidden state. */
export function RequireRole({ min, children }: { min: Role; children: React.ReactNode }) {
  return atLeast(useSession().role, min) ? <>{children}</> : <Forbidden />;
}
