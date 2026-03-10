import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { demoActiveAssets, demoFeedStatus } from '../../../shared/api/demo-fixtures'

// Mock the query hooks so we never need a real QueryClientProvider
vi.mock('../../../shared/api/queries', () => ({
  FOREGROUND_INTERVAL_MS: 1_000,
  BACKGROUND_INTERVAL_MS: 5_000,
  useFeedStatus: vi.fn(),
  useActiveAssets: vi.fn(),
  useOrderBookSnapshot: vi.fn(),
}))

import { useActiveAssets, useFeedStatus, useOrderBookSnapshot } from '../../../shared/api/queries'
import LiveFeedPage from '../LiveFeedPage'

const mockUseFeedStatus = vi.mocked(useFeedStatus)
const mockUseActiveAssets = vi.mocked(useActiveAssets)
const mockUseOrderBookSnapshot = vi.mocked(useOrderBookSnapshot)

// Helper to build a minimal UseQueryResult-shaped object
function queryResult<T>(
  overrides: Partial<{
    data: T
    isLoading: boolean
    error: Error | null
    isFetching: boolean
    dataUpdatedAt: number
  }> = {},
) {
  return {
    data: undefined as T | undefined,
    isLoading: false,
    error: null,
    isFetching: false,
    dataUpdatedAt: 0,
    isError: false,
    isPending: false,
    isSuccess: true,
    status: 'success' as const,
    fetchStatus: 'idle' as const,
    ...overrides,
    // biome-ignore lint/suspicious/noExplicitAny: test mock helper
  } as any
}

function assetQueryResult<T>(
  overrides: Partial<{
    data: T
    isLoading: boolean
    error: Error | null
    isFetching: boolean
  }> = {},
) {
  return {
    data: undefined as T | undefined,
    isLoading: false,
    error: null,
    isFetching: false,
    isError: false,
    isPending: false,
    isSuccess: true,
    status: 'success' as const,
    fetchStatus: 'idle' as const,
    ...overrides,
    // biome-ignore lint/suspicious/noExplicitAny: test mock helper
  } as any
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
afterEach(cleanup)

describe('LiveFeedPage', () => {
  it('renders feed status metrics with demo data', () => {
    mockUseFeedStatus.mockReturnValue(queryResult({ data: demoFeedStatus }))
    mockUseActiveAssets.mockReturnValue(assetQueryResult({ data: demoActiveAssets }))
    mockUseOrderBookSnapshot.mockReturnValue(
      queryResult({ data: undefined, isLoading: true }) as ReturnType<typeof useOrderBookSnapshot>,
    )

    render(<LiveFeedPage />)

    // Hero badge shows the session status
    expect(screen.getByText('Connected')).toBeInTheDocument()

    // MetricCard labels
    expect(screen.getByText('Feed mode')).toBeInTheDocument()
    expect(screen.getByText('Active assets')).toBeInTheDocument()
    expect(screen.getByText('Session ID')).toBeInTheDocument()
    expect(screen.getByText('Last rotation')).toBeInTheDocument()

    // MetricCard values derived from demo data
    expect(screen.getByText('Auto Rotate')).toBeInTheDocument()
    expect(screen.getByText('2')).toBeInTheDocument()
    expect(screen.getByText('ws-session-demo-btc-5m')).toBeInTheDocument()
  })

  it('renders asset cards from demo data', () => {
    mockUseFeedStatus.mockReturnValue(queryResult({ data: demoFeedStatus }))
    mockUseActiveAssets.mockReturnValue(assetQueryResult({ data: demoActiveAssets }))
    mockUseOrderBookSnapshot.mockReturnValue(
      queryResult({ data: undefined, isLoading: true }) as ReturnType<typeof useOrderBookSnapshot>,
    )

    render(<LiveFeedPage />)

    // Both demo assets should appear (btc-5m-yes appears in both asset list and Quick View header)
    expect(screen.getAllByText('btc-5m-yes').length).toBeGreaterThanOrEqual(1)
    expect(screen.getByText('btc-5m-no')).toBeInTheDocument()

    // Both have books and are fresh
    const bookBadges = screen.getAllByText('Book ready')
    expect(bookBadges).toHaveLength(2)

    const freshBadges = screen.getAllByText('Fresh')
    expect(freshBadges).toHaveLength(2)
  })

  it('shows loading text when queries are loading', () => {
    mockUseFeedStatus.mockReturnValue(
      queryResult({
        data: undefined,
        isLoading: true,
        isPending: true,
        isSuccess: false,
        status: 'pending',
      } as never),
    )
    mockUseActiveAssets.mockReturnValue(
      assetQueryResult({
        data: undefined,
        isLoading: true,
        isPending: true,
        isSuccess: false,
        status: 'pending',
      } as never),
    )
    mockUseOrderBookSnapshot.mockReturnValue(
      queryResult({ data: undefined, isLoading: true }) as ReturnType<typeof useOrderBookSnapshot>,
    )

    render(<LiveFeedPage />)

    // When feed data is not yet loaded, the MetricCard for Feed mode shows "Loading..."
    expect(screen.getByText('Loading...')).toBeInTheDocument()

    // When assets are loading and empty, a loading message appears
    expect(screen.getByText('Loading active assets...')).toBeInTheDocument()
  })
})
