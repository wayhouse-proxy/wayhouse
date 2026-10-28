import * as RadixTooltip from "@radix-ui/react-tooltip";

export const TooltipProvider = RadixTooltip.Provider;

/** Explains a disabled control or names an icon; never the only place a fact lives. */
export function Tooltip({ label, children }: { label: string; children: React.ReactElement }) {
  return (
    <RadixTooltip.Root>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content
          sideOffset={4}
          className="z-50 max-w-64 rounded-lg border border-line bg-surface px-2 py-1 text-xs text-ink shadow-overlay"
        >
          {label}
        </RadixTooltip.Content>
      </RadixTooltip.Portal>
    </RadixTooltip.Root>
  );
}
