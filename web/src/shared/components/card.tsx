import type { ReactNode } from 'react'

interface CardProps {
  children: ReactNode
  className?: string
  dense?: boolean
}

interface CardHeaderProps {
  title: string
  children?: ReactNode
}

export function Card({ children, className = '', dense = false }: CardProps) {
  return (
    <section
      className={`rounded-xl border border-card-border bg-card shadow-lg ${
        dense ? 'p-[var(--density-padding-sm)]' : 'p-[var(--density-padding)]'
      } ${className}`}
    >
      {children}
    </section>
  )
}

export function CardHeader({ title, children }: CardHeaderProps) {
  return (
    <div className="mb-[var(--density-gap)] flex items-center justify-between gap-3">
      <h3 className="m-0 text-base font-semibold text-foreground">{title}</h3>
      {children}
    </div>
  )
}
