import { forwardRef, type SelectHTMLAttributes } from 'react'

interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  label?: string
  options: { value: string; label: string }[]
}

export const Select = forwardRef<HTMLSelectElement, SelectProps>(
  ({ label, options, className = '', id, ...props }, ref) => {
    const selectId = id ?? label?.toLowerCase().replace(/\s+/g, '-')
    const select = (
      <select
        ref={ref}
        id={selectId}
        className={`w-full rounded-md border border-input-border bg-background px-3.5 py-3 text-foreground focus:outline-none focus:ring-2 focus:ring-ring ${className}`}
        {...props}
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value}>
            {opt.label}
          </option>
        ))}
      </select>
    )

    if (!label) return select

    return (
      <label
        htmlFor={selectId}
        className="grid gap-2 text-[var(--density-font-size)] text-foreground"
      >
        <span>{label}</span>
        {select}
      </label>
    )
  },
)

Select.displayName = 'Select'
