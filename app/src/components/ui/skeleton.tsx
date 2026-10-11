import { cn } from "@/lib/utils"

/** Loading placeholder. Pulses gently; static under prefers-reduced-motion. */
function Skeleton({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      aria-hidden="true"
      className={cn("shell-skeleton rounded-md", className)}
      {...props}
    />
  )
}

export { Skeleton }
