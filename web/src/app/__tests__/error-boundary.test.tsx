import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ErrorBoundary } from '../error-boundary'

afterEach(cleanup)

function ThrowingComponent({ message }: { message: string }): React.ReactNode {
  throw new Error(message)
}

function GoodComponent() {
  return <p>All clear</p>
}

describe('ErrorBoundary', () => {
  // Suppress React error boundary console.error noise in test output
  const originalConsoleError = console.error
  beforeEach(() => {
    console.error = vi.fn()
  })
  afterEach(() => {
    console.error = originalConsoleError
  })

  it('renders children when no error', () => {
    render(
      <ErrorBoundary>
        <GoodComponent />
      </ErrorBoundary>,
    )
    expect(screen.getByText('All clear')).toBeInTheDocument()
  })

  it('renders default route-level fallback on error', () => {
    render(
      <ErrorBoundary>
        <ThrowingComponent message="test crash" />
      </ErrorBoundary>,
    )
    expect(screen.getByText('This section encountered an error')).toBeInTheDocument()
    expect(screen.getByText('test crash')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument()
  })

  it('renders root-level fallback when level="root"', () => {
    render(
      <ErrorBoundary level="root">
        <ThrowingComponent message="root crash" />
      </ErrorBoundary>,
    )
    expect(screen.getByText('Something went wrong')).toBeInTheDocument()
    expect(screen.getByText('root crash')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Reload page' })).toBeInTheDocument()
  })

  it('renders custom fallback when provided', () => {
    render(
      <ErrorBoundary fallback={<div>Custom error UI</div>}>
        <ThrowingComponent message="fallback crash" />
      </ErrorBoundary>,
    )
    expect(screen.getByText('Custom error UI')).toBeInTheDocument()
  })

  it('"Try again" button recovers from route-level error', async () => {
    const user = userEvent.setup()
    let shouldThrow = true

    function MaybeThrow(): React.ReactNode {
      if (shouldThrow) throw new Error('recoverable')
      return <p>Recovered</p>
    }

    render(
      <ErrorBoundary level="route">
        <MaybeThrow />
      </ErrorBoundary>,
    )

    expect(screen.getByText('This section encountered an error')).toBeInTheDocument()

    // Fix the error condition, then click try again
    shouldThrow = false
    await user.click(screen.getByRole('button', { name: 'Try again' }))

    expect(screen.getByText('Recovered')).toBeInTheDocument()
  })

  it('resets a caught error when resetKeys change (e.g. on navigation)', () => {
    const { rerender } = render(
      <ErrorBoundary resetKeys={['/a']}>
        <ThrowingComponent message="route a crashed" />
      </ErrorBoundary>,
    )
    expect(screen.getByText('route a crashed')).toBeInTheDocument()

    // Navigating to a different route changes resetKeys and renders a healthy
    // child — the boundary must recover instead of staying stuck on the error.
    rerender(
      <ErrorBoundary resetKeys={['/b']}>
        <GoodComponent />
      </ErrorBoundary>,
    )
    expect(screen.getByText('All clear')).toBeInTheDocument()
  })

  it('shows generic message when error has empty message', () => {
    function ThrowEmpty(): React.ReactNode {
      throw new Error()
    }

    render(
      <ErrorBoundary>
        <ThrowEmpty />
      </ErrorBoundary>,
    )
    expect(screen.getByText('An unexpected error occurred.')).toBeInTheDocument()
  })
})
