import { render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useAdaptivePolling } from './useAdaptivePolling'

function TestPolling({ visibleIntervalMs, hiddenIntervalMs }: { visibleIntervalMs: number; hiddenIntervalMs: number }) {
  const polling = useAdaptivePolling({
    visibleIntervalMs,
    hiddenIntervalMs,
    runTask: async () => undefined,
  })

  return (
    <div>
      <span data-testid="visibility">{polling.isVisible ? 'visible' : 'hidden'}</span>
      <span data-testid="interval">{polling.currentIntervalMs}</span>
      <span data-testid="calls">{polling.runCount}</span>
    </div>
  )
}

describe('useAdaptivePolling', () => {
  beforeEach(() => {
    Object.defineProperty(document, 'hidden', {
      configurable: true,
      value: false,
    })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('runs immediately and then keeps the visible cadence', async () => {
    render(<TestPolling hiddenIntervalMs={40} visibleIntervalMs={20} />)

    expect(screen.getByTestId('visibility')).toHaveTextContent('visible')
    expect(screen.getByTestId('interval')).toHaveTextContent('20')
    await waitFor(() => expect(screen.getByTestId('calls')).toHaveTextContent('1'))

    await waitFor(() => expect(screen.getByTestId('calls')).toHaveTextContent('2'))
  })

  it('switches to the hidden cadence after visibility changes', async () => {
    render(<TestPolling hiddenIntervalMs={40} visibleIntervalMs={20} />)

    expect(screen.getByTestId('visibility')).toHaveTextContent('visible')
    await waitFor(() => expect(screen.getByTestId('calls')).toHaveTextContent('1'))
    Object.defineProperty(document, 'hidden', {
      configurable: true,
      value: true,
    })
    document.dispatchEvent(new Event('visibilitychange'))

    await waitFor(() => expect(screen.getByTestId('visibility')).toHaveTextContent('hidden'))
    expect(screen.getByTestId('interval')).toHaveTextContent('40')
    await waitFor(() => expect(screen.getByTestId('calls')).toHaveTextContent('2'))
    await waitFor(() => expect(screen.getByTestId('calls')).toHaveTextContent('3'))
  })
})
