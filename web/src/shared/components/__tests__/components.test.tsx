import type { ColumnDef } from '@tanstack/react-table'
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { Badge } from '../badge'
import { Button } from '../button'
import { Card, CardHeader } from '../card'
import { DataTable } from '../data-table'
import { ErrorBanner } from '../error-banner'

afterEach(cleanup)

// ---------------------------------------------------------------------------
// Card
// ---------------------------------------------------------------------------
describe('Card', () => {
  it('renders children', () => {
    render(<Card>Card content</Card>)
    expect(screen.getByText('Card content')).toBeInTheDocument()
  })

  it('renders with CardHeader title', () => {
    render(
      <Card>
        <CardHeader title="My Title" />
      </Card>,
    )
    expect(screen.getByText('My Title')).toBeInTheDocument()
  })

  it('applies className', () => {
    render(<Card className="custom-class">Content</Card>)
    const section = screen.getByText('Content').closest('section')
    expect(section).toHaveClass('custom-class')
  })
})

// ---------------------------------------------------------------------------
// Badge
// ---------------------------------------------------------------------------
describe('Badge', () => {
  it('renders text', () => {
    render(<Badge>Active</Badge>)
    expect(screen.getByText('Active')).toBeInTheDocument()
  })

  it('applies success variant classes', () => {
    render(<Badge variant="success">OK</Badge>)
    const el = screen.getByText('OK')
    expect(el).toHaveClass('text-success')
  })

  it('applies warning variant classes', () => {
    render(<Badge variant="warning">Warn</Badge>)
    const el = screen.getByText('Warn')
    expect(el).toHaveClass('text-warning')
  })

  it('applies error variant classes', () => {
    render(<Badge variant="error">Fail</Badge>)
    const el = screen.getByText('Fail')
    expect(el).toHaveClass('text-destructive')
  })

  it('applies neutral variant classes by default', () => {
    render(<Badge>Default</Badge>)
    const el = screen.getByText('Default')
    expect(el).toHaveClass('text-muted-foreground')
  })
})

// ---------------------------------------------------------------------------
// Button
// ---------------------------------------------------------------------------
describe('Button', () => {
  it('renders text', () => {
    render(<Button>Click me</Button>)
    expect(screen.getByRole('button', { name: 'Click me' })).toBeInTheDocument()
  })

  it('handles click', async () => {
    const user = userEvent.setup()
    const handleClick = vi.fn()
    render(<Button onClick={handleClick}>Press</Button>)
    await user.click(screen.getByRole('button', { name: 'Press' }))
    expect(handleClick).toHaveBeenCalledOnce()
  })

  it('does not fire click when disabled', async () => {
    const user = userEvent.setup()
    const handleClick = vi.fn()
    render(
      <Button onClick={handleClick} disabled>
        No
      </Button>,
    )
    await user.click(screen.getByRole('button', { name: 'No' }))
    expect(handleClick).not.toHaveBeenCalled()
  })

  it('applies secondary variant classes', () => {
    render(<Button variant="secondary">Secondary</Button>)
    const btn = screen.getByRole('button', { name: 'Secondary' })
    expect(btn).toHaveClass('text-accent')
  })

  it('applies ghost variant classes', () => {
    render(<Button variant="ghost">Ghost</Button>)
    const btn = screen.getByRole('button', { name: 'Ghost' })
    expect(btn).toHaveClass('bg-transparent')
  })
})

// ---------------------------------------------------------------------------
// DataTable
// ---------------------------------------------------------------------------
interface TestRow {
  id: number
  name: string
  value: number
}

const testColumns: ColumnDef<TestRow, unknown>[] = [
  { accessorKey: 'id', header: 'ID' },
  { accessorKey: 'name', header: 'Name' },
  { accessorKey: 'value', header: 'Value' },
]

const testData: TestRow[] = [
  { id: 1, name: 'Alpha', value: 100 },
  { id: 2, name: 'Beta', value: 200 },
]

describe('DataTable', () => {
  it('renders column headers', () => {
    render(<DataTable columns={testColumns} data={testData} />)
    expect(screen.getByText('ID')).toBeInTheDocument()
    expect(screen.getByText('Name')).toBeInTheDocument()
    expect(screen.getByText('Value')).toBeInTheDocument()
  })

  it('renders data rows', () => {
    render(<DataTable columns={testColumns} data={testData} />)
    expect(screen.getByText('Alpha')).toBeInTheDocument()
    expect(screen.getByText('Beta')).toBeInTheDocument()
    expect(screen.getByText('100')).toBeInTheDocument()
    expect(screen.getByText('200')).toBeInTheDocument()
  })

  it('handles empty data', () => {
    render(<DataTable columns={testColumns} data={[]} />)
    expect(screen.getByText('ID')).toBeInTheDocument()
    // No data rows — only headers should exist
    expect(screen.queryByText('Alpha')).not.toBeInTheDocument()
  })
})

// ---------------------------------------------------------------------------
// ErrorBanner
// ---------------------------------------------------------------------------
describe('ErrorBanner', () => {
  it('renders title and message', () => {
    render(<ErrorBanner title="Oops" message="Something went wrong" />)
    expect(screen.getByText('Oops')).toBeInTheDocument()
    expect(screen.getByText('Something went wrong')).toBeInTheDocument()
  })

  it('has role="alert"', () => {
    render(<ErrorBanner title="Error" message="Bad request" />)
    expect(screen.getByRole('alert')).toBeInTheDocument()
  })

  it('renders hint when provided', () => {
    render(<ErrorBanner title="Error" message="Fail" hint="Try again later" />)
    expect(screen.getByText('Try again later')).toBeInTheDocument()
  })

  it('does not render hint when not provided', () => {
    render(<ErrorBanner title="Error" message="Fail" />)
    // Only two paragraphs: title (strong) and message (p)
    const alert = screen.getByRole('alert')
    const paragraphs = alert.querySelectorAll('p')
    expect(paragraphs).toHaveLength(1)
  })
})
