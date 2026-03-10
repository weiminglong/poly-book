import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { demoDatasets } from '../../../shared/api/demo-fixtures'

// Mock the query hooks so we never need a real QueryClientProvider
vi.mock('../../../shared/api/queries', () => ({
  useDatasets: vi.fn(),
  useQuerySql: vi.fn(),
}))

import { useDatasets, useQuerySql } from '../../../shared/api/queries'
import QueryPage from '../QueryPage'

const mockUseDatasets = vi.mocked(useDatasets)
const mockUseQuerySql = vi.mocked(useQuerySql)

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
    isError: false,
    isPending: false,
    isSuccess: true,
    status: 'success' as const,
    fetchStatus: 'idle' as const,
    refetch: vi.fn(),
    ...overrides,
  } as ReturnType<typeof useDatasets>
}

function mutationResult(
  overrides: Partial<{ data: unknown; error: Error | null; isPending: boolean }> = {},
) {
  return {
    data: undefined,
    error: null,
    isPending: false,
    isError: false,
    isIdle: true,
    isSuccess: false,
    status: 'idle' as const,
    mutate: vi.fn(),
    mutateAsync: vi.fn(),
    reset: vi.fn(),
    variables: undefined,
    submittedAt: 0,
    ...overrides,
  } as unknown as ReturnType<typeof useQuerySql>
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
afterEach(cleanup)

describe('QueryPage', () => {
  it('renders the page heading', () => {
    mockUseDatasets.mockReturnValue(queryResult({ data: demoDatasets }))
    mockUseQuerySql.mockReturnValue(mutationResult())

    render(<QueryPage />)

    expect(screen.getByText('Query Workbench')).toBeInTheDocument()
    expect(screen.getByText('Read-only SQL over split datasets.')).toBeInTheDocument()
  })

  it('renders schema browser with dataset names', () => {
    mockUseDatasets.mockReturnValue(queryResult({ data: demoDatasets }))
    mockUseQuerySql.mockReturnValue(mutationResult())

    render(<QueryPage />)

    // The SchemaBrowser renders a "Datasets" header
    expect(screen.getByText('Datasets')).toBeInTheDocument()

    // Demo datasets appear as expandable buttons
    for (const dataset of demoDatasets.datasets) {
      expect(screen.getAllByText(dataset.name).length).toBeGreaterThanOrEqual(1)
    }
  })

  it('renders the SQL editor textarea', () => {
    mockUseDatasets.mockReturnValue(queryResult({ data: demoDatasets }))
    mockUseQuerySql.mockReturnValue(mutationResult())

    render(<QueryPage />)

    // The textarea should be present
    const textarea = screen.getByRole('textbox')
    expect(textarea).toBeInTheDocument()
    expect(textarea.tagName).toBe('TEXTAREA')

    // The Run Query button
    expect(screen.getByRole('button', { name: 'Run Query' })).toBeInTheDocument()
  })
})
