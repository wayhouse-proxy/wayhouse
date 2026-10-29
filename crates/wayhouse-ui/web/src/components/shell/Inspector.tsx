import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import { Button } from "../ui/Button";

interface Open {
  title: string;
  content: React.ReactNode;
}

interface Ctx {
  /** Shows details for the selected row; focus moves to the heading and returns to the opener on close. */
  open: (title: string, content: React.ReactNode) => void;
  close: () => void;
  current: Open | null;
}

const InspectorCtx = createContext<Ctx>({ open: () => {}, close: () => {}, current: null });
export const useInspector = () => useContext(InspectorCtx);

export function InspectorProvider({ children }: { children: React.ReactNode }) {
  const [current, setCurrent] = useState<Open | null>(null);
  const opener = useRef<HTMLElement | null>(null);

  const open = useCallback((title: string, content: React.ReactNode) => {
    // Selecting another row replaces the content but keeps the original opener to return to.
    if (!opener.current && document.activeElement instanceof HTMLElement) opener.current = document.activeElement;
    setCurrent({ title, content });
  }, []);
  const close = useCallback(() => {
    setCurrent(null);
    opener.current?.focus();
    opener.current = null;
  }, []);

  return <InspectorCtx.Provider value={{ open, close, current }}>{children}</InspectorCtx.Provider>;
}

/** The right-hand panel, 20 rem; it sits below the page under 1100 px. Renders nothing until something opens it. */
export function InspectorPanel() {
  const { current, close } = useInspector();
  const heading = useRef<HTMLHeadingElement>(null);
  const title = current?.title;
  useEffect(() => {
    if (title !== undefined) heading.current?.focus();
  }, [title]);

  if (!current) return null;
  return (
    <aside
      aria-label="Details"
      onKeyDown={(e) => e.key === "Escape" && close()}
      className="shrink-0 border-t border-line bg-surface min-[1100px]:w-(--inspector-width) min-[1100px]:overflow-y-auto min-[1100px]:border-t-0 min-[1100px]:border-l"
    >
      <div className="flex items-center justify-between gap-2 border-b border-line px-3 py-2">
        <h2 ref={heading} tabIndex={-1} className="font-mono text-sm font-semibold break-all outline-none">
          {current.title}
        </h2>
        <Button variant="ghost" size="sm" onClick={close} aria-label="Close details">
          <X aria-hidden size={16} strokeWidth={1.5} />
        </Button>
      </div>
      <div className="p-3">{current.content}</div>
    </aside>
  );
}
