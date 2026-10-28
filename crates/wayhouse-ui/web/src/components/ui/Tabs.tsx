import * as RadixTabs from "@radix-ui/react-tabs";
import { cn } from "../../lib/cn";

export const Tabs = RadixTabs.Root;
export const TabsContent = RadixTabs.Content;

/** Text tabs with a 2 px selection underline; arrow keys move (Radix). They split one subject, they are not steps. */
export function TabsList({ className, ...props }: React.ComponentProps<typeof RadixTabs.List>) {
  return <RadixTabs.List className={cn("flex gap-4 border-b border-line", className)} {...props} />;
}

export function TabsTrigger({ className, ...props }: React.ComponentProps<typeof RadixTabs.Trigger>) {
  return (
    <RadixTabs.Trigger
      className={cn(
        `-mb-px min-h-(--control-height) border-b-2 border-transparent px-1 text-sm text-ink-muted
         transition-colors duration-(--dur-fast) hover:text-ink data-[state=active]:border-select
         data-[state=active]:font-semibold data-[state=active]:text-ink disabled:opacity-50`,
        className,
      )}
      {...props}
    />
  );
}
