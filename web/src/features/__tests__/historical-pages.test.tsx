import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { demoActiveAssets } from '../../shared/api/demo-fixtures'

// Mock the query hooks so we never need a real QueryClientProvider
vi.mock('../../shared/api/queries', () => ({
  FOREGROUND_INTERVAL_MS: 1_000,
  BACKGROUND_INTERVAL_MS: 5_000,
  useActiveAssets: vi.fn(),
  useReplayReconstruction: vi.fn(),
  useExecutionTimeline: vi.fn(),
  useIntegritySummary: vi.fn(),
}))

import {
  useActiveAssets,
  useExecutionTimeline,
  useIntegritySummary,
  useReplayReconstruction,
} from '../../shared/api/queries'
import ExecutionPage from '../execution/ExecutionPage'
import IntegrityPage from '../integrity/IntegrityPage'
import ReplayPage from '../replay/ReplayPage'

const mockUseActiveAssets = vi.mocked(useActiveAssets)
const mockUseReplayReconstruction = vi.mocked(useReplayReconstruction)
const mockUseExecutionTimeline = vi.mocked(useExecutionTimeline)
const mockUseIntegritySummary = vi.mocked(useIntegritySummary)

// Helper to build a minimal UseQueryResult-shaped object
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
  } as unknown as ReturnType<typeof useActiveAssets>
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
afterEach(cleanup)

describe('ReplayPage', () => {
  function setupReplayMocks() {
    mockUseActiveAssets.mockReturnValue(queryResult({ data: demoActiveAssets }))
    mockUseReplayReconstruction.mockReturnValue(queryResult({ data: undefined }))
  }

  it('renders the page heading "Replay Workbench"', () => {
    setupReplayMocks()
    render(<ReplayPage />)
    expect(screen.getByText('Replay Workbench')).toBeInTheDocument()
  })

  it('renders the query form with Asset ID input and Run button', () => {
    setupReplayMocks()
    render(<ReplayPage />)
    expect(screen.getByLabelText('Asset ID')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Run Reconstruction' })).toBeInTheDocument()
  })
})

describe('ExecutionPage', () => {
  function setupExecutionMocks() {
    mockUseExecutionTimeline.mockReturnValue(queryResult({ data: undefined }))
  }

  it('renders the page heading "Execution Inspector"', () => {
    setupExecutionMocks()
    render(<ExecutionPage />)
    expect(screen.getByText('Execution Inspector')).toBeInTheDocument()
  })

  it('renders the time window form inputs', () => {
    setupExecutionMocks()
    render(<ExecutionPage />)
    expect(screen.getByLabelText('Window (minutes)')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Query' })).toBeInTheDocument()
  })
})

describe('IntegrityPage', () => {
  function setupIntegrityMocks() {
    mockUseIntegritySummary.mockReturnValue(queryResult({ data: undefined }))
  }

  it('renders the page heading "Integrity"', () => {
    setupIntegrityMocks()
    render(<IntegrityPage />)
    expect(screen.getByText('Integrity')).toBeInTheDocument()
  })

  it('renders the time window form inputs', () => {
    setupIntegrityMocks()
    render(<IntegrityPage />)
    expect(screen.getByLabelText('Window (minutes)')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Query' })).toBeInTheDocument()
  })
})
