import { type ButtonHTMLAttributes, forwardRef } from 'react'

type ButtonVariant = 'primary' | 'secondary' | 'ghost'
type ButtonSize = 'sm' | 'md' | 'lg'

const variantClasses: Record<ButtonVariant, string> = {
  primary:
    'bg-gradient-to-br from-teal-700 to-cyan-600 text-white font-bold border-0 hover:-translate-y-0.5 transition-transform',
  secondary:
    'bg-ring/12 border border-ring/35 text-accent hover:-translate-y-0.5 transition-transform',
  ghost:
    'bg-transparent border border-transparent text-muted-foreground hover:bg-muted hover:text-foreground transition-colors',
}

const sizeClasses: Record<ButtonSize, string> = {
  sm: 'px-3 py-1.5 text-sm rounded-lg',
  md: 'px-4 py-2.5 text-sm rounded-xl',
  lg: 'px-6 py-3 text-base rounded-xl',
}

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant
  size?: ButtonSize
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ variant = 'primary', size = 'md', className = '', ...props }, ref) => {
    return (
      <button
        ref={ref}
        className={`inline-flex items-center justify-center gap-2 ${variantClasses[variant]} ${sizeClasses[size]} disabled:pointer-events-none disabled:opacity-50 ${className}`}
        {...props}
      />
    )
  },
)

Button.displayName = 'Button'
