export function Skeleton({ className = '' }: { className?: string }) {
  return <div className={`animate-pulse rounded-md bg-muted ${className}`} aria-hidden="true" />
}

export function CardSkeleton() {
  return (
    <div className="rounded-xl border border-card-border bg-card p-[var(--density-padding)]">
      <Skeleton className="mb-4 h-5 w-1/3" />
      <div className="grid gap-3">
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    </div>
  )
}
