import * as RadixToast from "@radix-ui/react-toast";
import { createContext, useCallback, useContext, useState } from "react";
import { StatusMark } from "./StatusMark";

type Tone = "up" | "down";
interface Item {
  id: number;
  text: string;
  tone: Tone;
}

const Ctx = createContext<(text: string, tone?: Tone) => void>(() => {});

/** `useToast()("Dashboard saved")` announces a finished action (a polite live region, so screen readers hear it). */
export const useToast = () => useContext(Ctx);

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [items, setItems] = useState<Item[]>([]);
  const push = useCallback((text: string, tone: Tone = "up") => {
    setItems((all) => [...all, { id: Date.now() + all.length, text, tone }]);
  }, []);
  return (
    <Ctx.Provider value={push}>
      <RadixToast.Provider duration={5000} label="Notifications">
        {children}
        {items.map((t) => (
          <RadixToast.Root
            key={t.id}
            type={t.tone === "down" ? "foreground" : "background"}
            onOpenChange={(open) => !open && setItems((all) => all.filter((x) => x.id !== t.id))}
            className="rounded-lg border border-line bg-surface px-3 py-2 text-sm shadow-overlay"
          >
            <RadixToast.Description>
              <StatusMark state={t.tone} word={t.text} />
            </RadixToast.Description>
          </RadixToast.Root>
        ))}
        <RadixToast.Viewport className="fixed right-4 bottom-4 z-50 flex w-80 max-w-[calc(100vw-2rem)] flex-col gap-2 outline-none" />
      </RadixToast.Provider>
    </Ctx.Provider>
  );
}
