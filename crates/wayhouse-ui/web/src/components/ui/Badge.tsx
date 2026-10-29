import { StatusMark, type State } from "./StatusMark";

const STATES: Record<string, State> = { good: "up", warn: "degraded", bad: "down", neutral: "unknown" };

/** A state as shape, colour and word, always together (never colour alone). */
export function Badge({ tone, children }: { tone: keyof typeof STATES; children: React.ReactNode }) {
  return <StatusMark state={STATES[tone]} word={children} />;
}
