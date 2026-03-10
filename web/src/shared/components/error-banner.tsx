export function ErrorBanner({
  title,
  message,
  hint,
}: {
  title: string
  message: string
  hint?: string
}) {
  return (
    <div
      role="alert"
      className="grid gap-2 rounded-xl border border-[rgba(248,113,113,0.35)] bg-[rgba(127,29,29,0.22)] p-[var(--density-padding-sm)]"
    >
      <strong className="text-foreground">{title}</strong>
      <p className="m-0 text-destructive">{message}</p>
      {hint ? <p className="m-0 text-muted-foreground">{hint}</p> : null}
    </div>
  )
}
