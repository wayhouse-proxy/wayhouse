import * as RadixSwitch from "@radix-ui/react-switch";
import { cn } from "../../lib/cn";

/** For boolean settings only: 34 by 18 px, on = the selection colour. */
export function Switch({ className, ...props }: React.ComponentProps<typeof RadixSwitch.Root>) {
  return (
    <RadixSwitch.Root
      className={cn(
        `relative h-[18px] w-[34px] shrink-0 rounded-full border border-control bg-surface-sunken
         transition-colors duration-(--dur-fast) hover:border-ink-muted data-[state=checked]:border-select
         data-[state=checked]:bg-select disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-bad`,
        className,
      )}
      {...props}
    >
      <RadixSwitch.Thumb
        className="block h-3 w-3 translate-x-[2px] rounded-full bg-ink-muted transition-transform
          duration-(--dur-fast) data-[state=checked]:translate-x-[18px] data-[state=checked]:bg-surface"
      />
    </RadixSwitch.Root>
  );
}
