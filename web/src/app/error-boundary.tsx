import { Component, type ErrorInfo, type ReactNode } from 'react'

interface ErrorBoundaryProps {
  fallback?: ReactNode
  level?: 'root' | 'route'
  /** When any value here changes, a caught error is cleared. Pass the current
   *  route (e.g. the pathname) so navigating away from a broken page recovers
   *  instead of the error UI bricking all navigation (A.71). */
  resetKeys?: unknown[]
  children: ReactNode
}

interface ErrorBoundaryState {
  hasError: boolean
  error: Error | null
}

function resetKeysChanged(a: unknown[] | undefined, b: unknown[] | undefined): boolean {
  if (a === b) return false
  if (!a || !b || a.length !== b.length) return true
  return a.some((value, index) => !Object.is(value, b[index]))
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props)
    this.state = { hasError: false, error: null }
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('[ErrorBoundary]', error, info.componentStack)
  }

  componentDidUpdate(prevProps: ErrorBoundaryProps) {
    if (this.state.hasError && resetKeysChanged(prevProps.resetKeys, this.props.resetKeys)) {
      this.setState({ hasError: false, error: null })
    }
  }

  render() {
    if (!this.state.hasError) {
      return this.props.children
    }

    if (this.props.fallback) {
      return this.props.fallback
    }

    const isRoot = this.props.level === 'root'

    return (
      <div
        className={`grid place-items-center ${isRoot ? 'min-h-screen' : 'min-h-[300px]'} p-8`}
        role="alert"
      >
        <div className="grid max-w-md gap-4 text-center">
          <h2 className="text-xl font-semibold text-foreground">
            {isRoot ? 'Something went wrong' : 'This section encountered an error'}
          </h2>
          <p className="text-muted-foreground">
            {this.state.error?.message || 'An unexpected error occurred.'}
          </p>
          <button
            type="button"
            onClick={() => {
              if (isRoot) {
                window.location.reload()
              } else {
                this.setState({ hasError: false, error: null })
              }
            }}
            className="mx-auto inline-flex items-center gap-2 rounded-xl bg-gradient-to-br from-teal-700 to-cyan-600 px-6 py-3 font-bold text-white"
          >
            {isRoot ? 'Reload page' : 'Try again'}
          </button>
        </div>
      </div>
    )
  }
}
