export function MetricCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid gap-1.5 rounded-lg border border-card-border bg-card p-[var(--density-padding-sm)]">
      <span className="text-[var(--density-font-size)] text-muted-foreground">{label}</span>
      <strong className="text-lg text-foreground">{value}</strong>
    </div>
  )
}
