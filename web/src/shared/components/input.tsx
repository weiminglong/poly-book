import { forwardRef, type InputHTMLAttributes } from 'react'

interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  label?: string
}

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ label, className = '', id, ...props }, ref) => {
    const inputId = id ?? label?.toLowerCase().replace(/\s+/g, '-')
    const input = (
      <input
        ref={ref}
        id={inputId}
        className={`w-full rounded-md border border-input-border bg-background px-3.5 py-3 text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring ${className}`}
        {...props}
      />
    )

    if (!label) return input

    return (
      <label
        htmlFor={inputId}
        className="grid gap-2 text-[var(--density-font-size)] text-foreground"
      >
        <span>{label}</span>
        {input}
      </label>
    )
  },
)

Input.displayName = 'Input'
