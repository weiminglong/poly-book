import * as TooltipPrimitive from '@radix-ui/react-tooltip'
import type { ReactNode } from 'react'

export function TooltipProvider({ children }: { children: ReactNode }) {
  return <TooltipPrimitive.Provider delayDuration={300}>{children}</TooltipPrimitive.Provider>
}

export function Tooltip({
  children,
  content,
  side = 'top',
}: {
  children: ReactNode
  content: ReactNode
  side?: 'top' | 'bottom' | 'left' | 'right'
}) {
  return (
    <TooltipPrimitive.Root>
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipPrimitive.Portal>
        <TooltipPrimitive.Content
          side={side}
          sideOffset={6}
          className="z-50 rounded-lg border border-card-border bg-card px-3 py-2 text-sm text-foreground shadow-md animate-in fade-in-0 zoom-in-95"
        >
          {content}
          <TooltipPrimitive.Arrow className="fill-card" />
        </TooltipPrimitive.Content>
      </TooltipPrimitive.Portal>
    </TooltipPrimitive.Root>
  )
}
