import type { ReactNode } from 'react'

type BadgeVariant = 'success' | 'warning' | 'error' | 'neutral' | 'accent'

const variantClasses: Record<BadgeVariant, string> = {
  success: 'bg-bid-bg text-success',
  warning: 'bg-warning/15 text-warning',
  error: 'bg-ask-bg text-destructive',
  neutral: 'bg-muted text-muted-foreground',
  accent: 'bg-accent/18 text-accent',
}

export function Badge({
  variant = 'neutral',
  children,
}: {
  variant?: BadgeVariant
  children: ReactNode
}) {
  return (
    <span
      className={`inline-flex items-center self-start rounded-full px-3 py-1.5 text-xs font-bold ${variantClasses[variant]}`}
    >
      {children}
    </span>
  )
}
