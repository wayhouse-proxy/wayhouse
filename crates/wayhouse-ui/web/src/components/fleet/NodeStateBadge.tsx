import type { NodeState } from "../../lib/fleet/nodes";
import { Badge } from "../ui/Badge";

const TONE = { ok: "good", degraded: "warn", stale: "neutral" } as const;
const WORD = { ok: "ok", degraded: "degraded", stale: "stale" } as const;

export const NodeStateBadge = ({ state }: { state: NodeState }) => <Badge tone={TONE[state]}>{WORD[state]}</Badge>;
