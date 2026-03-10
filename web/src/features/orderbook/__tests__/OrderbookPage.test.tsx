import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { demoActiveAssets, getDemoSnapshot } from '../../../shared/api/demo-fixtures'

// Mock the query hooks
vi.mock('../../../shared/api/queries', () => ({
  FOREGROUND_INTERVAL_MS: 1_000,
  BACKGROUND_INTERVAL_MS: 5_000,
  useActiveAssets: vi.fn(),
  useOrderBookSnapshot: vi.fn(),
  queryKeys: { orderbook: (assetId: string, depth: number) => ['orderbook', assetId, depth] },
}))

// Mock the orderbook stream hook
vi.mock('../../../shared/hooks/use-orderbook-stream', () => ({
  useOrderBookStream: vi.fn(),
}))

import { useActiveAssets, useOrderBookSnapshot } from '../../../shared/api/queries'
import { useOrderBookStream } from '../../../shared/hooks/use-orderbook-stream'
import OrderbookPage from '../OrderbookPage'

const mockUseActiveAssets = vi.mocked(useActiveAssets)
const mockUseOrderBookSnapshot = vi.mocked(useOrderBookSnapshot)
const mockUseOrderBookStream = vi.mocked(useOrderBookStream)

function queryResult<T>(
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

function setupDefaultMocks(snapshotOverrides?: Parameters<typeof queryResult>[0]) {
  mockUseActiveAssets.mockReturnValue(queryResult({ data: demoActiveAssets }))
  mockUseOrderBookSnapshot.mockReturnValue(
    queryResult({ data: getDemoSnapshot('btc-5m-yes', 10), ...snapshotOverrides }) as ReturnType<
      typeof useOrderBookSnapshot
    >,
  )
  mockUseOrderBookStream.mockReturnValue({
    snapshot: null,
    status: 'closed',
    error: null,
  })
}

afterEach(cleanup)

describe('OrderbookPage', () => {
  it('renders the page heading', () => {
    setupDefaultMocks()
    render(<OrderbookPage />)
    expect(screen.getByText('Orderbook')).toBeInTheDocument()
  })

  it('renders asset selector buttons from active assets', () => {
    setupDefaultMocks()
    render(<OrderbookPage />)
    expect(screen.getByRole('button', { name: 'btc-5m-yes' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'btc-5m-no' })).toBeInTheDocument()
  })

  it('renders depth preset buttons', () => {
    setupDefaultMocks()
    render(<OrderbookPage />)
    for (const d of [5, 10, 25, 50, 100, 200]) {
      expect(screen.getByRole('button', { name: String(d) })).toBeInTheDocument()
    }
  })

  it('renders metric cards when snapshot data is available', () => {
    setupDefaultMocks()
    render(<OrderbookPage />)
    expect(screen.getByText('Best bid')).toBeInTheDocument()
    expect(screen.getByText('Best ask')).toBeInTheDocument()
    expect(screen.getByText('Mid')).toBeInTheDocument()
    expect(screen.getByText('Spread')).toBeInTheDocument()
    expect(screen.getByText('Bid depth')).toBeInTheDocument()
    expect(screen.getByText('Ask depth')).toBeInTheDocument()
    expect(screen.getByText('Sequence')).toBeInTheDocument()
  })

  it('renders skeleton loading state', () => {
    mockUseActiveAssets.mockReturnValue(queryResult({ data: demoActiveAssets }))
    mockUseOrderBookSnapshot.mockReturnValue(
      queryResult({
        data: undefined,
        isLoading: true,
        isPending: true,
        isSuccess: false,
        status: 'pending',
      } as never) as ReturnType<typeof useOrderBookSnapshot>,
    )
    mockUseOrderBookStream.mockReturnValue({
      snapshot: null,
      status: 'closed',
      error: null,
    })

    render(<OrderbookPage />)
    // When loading, skeleton elements should exist (no metric cards)
    expect(screen.queryByText('Best bid')).not.toBeInTheDocument()
  })

  it('renders error banner when snapshot query fails', () => {
    mockUseActiveAssets.mockReturnValue(queryResult({ data: demoActiveAssets }))
    mockUseOrderBookSnapshot.mockReturnValue(
      queryResult({
        data: undefined,
        error: new Error('Network timeout'),
        isError: true,
        isSuccess: false,
      } as never) as ReturnType<typeof useOrderBookSnapshot>,
    )
    mockUseOrderBookStream.mockReturnValue({
      snapshot: null,
      status: 'closed',
      error: null,
    })

    render(<OrderbookPage />)
    expect(screen.getByText('Orderbook fetch failed')).toBeInTheDocument()
    expect(screen.getByText('Network timeout')).toBeInTheDocument()
  })

  it('shows WebSocket transport badge when connected', () => {
    setupDefaultMocks()
    mockUseOrderBookStream.mockReturnValue({
      snapshot: null,
      status: 'connected',
      error: null,
    })

    render(<OrderbookPage />)
    expect(screen.getByText('WebSocket')).toBeInTheDocument()
  })

  it('shows HTTP Fallback badge when stream falls back', () => {
    setupDefaultMocks()
    mockUseOrderBookStream.mockReturnValue({
      snapshot: null,
      status: 'fallback',
      error: 'WebSocket unavailable',
    })

    render(<OrderbookPage />)
    expect(screen.getByText('HTTP Fallback')).toBeInTheDocument()
  })

  it('switching depth preset changes the active button style', async () => {
    setupDefaultMocks()
    const user = userEvent.setup()

    render(<OrderbookPage />)

    const btn25 = screen.getByRole('button', { name: '25' })
    await user.click(btn25)

    // The clicked button should have the active style class
    expect(btn25).toHaveClass('font-bold')
  })

  it('switching asset changes the active asset button style', async () => {
    setupDefaultMocks()
    const user = userEvent.setup()

    render(<OrderbookPage />)

    const noBtn = screen.getByRole('button', { name: 'btc-5m-no' })
    await user.click(noBtn)

    expect(noBtn).toHaveClass('font-bold')
  })

  it('shows placeholder when no asset is selected and no assets available', () => {
    mockUseActiveAssets.mockReturnValue(queryResult({ data: [] }))
    mockUseOrderBookSnapshot.mockReturnValue(
      queryResult({ data: undefined }) as ReturnType<typeof useOrderBookSnapshot>,
    )
    mockUseOrderBookStream.mockReturnValue({
      snapshot: null,
      status: 'closed',
      error: null,
    })

    render(<OrderbookPage />)
    expect(screen.getByText('Select an asset to view the orderbook.')).toBeInTheDocument()
  })
})
