import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import { CommandPalette } from '../command-palette'

// cmdk internally uses ResizeObserver and Element.scrollIntoView which jsdom
// does not provide. Stub them out so the component can mount in tests.
beforeAll(() => {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  Element.prototype.scrollIntoView = vi.fn()
})

function renderPalette(props: Partial<React.ComponentProps<typeof CommandPalette>> = {}) {
  const defaults = {
    onToggleTheme: vi.fn(),
    onSetDensity: vi.fn(),
    onSetSource: vi.fn(),
  }
  return {
    ...defaults,
    ...render(
      <MemoryRouter>
        <CommandPalette {...defaults} {...props} />
      </MemoryRouter>,
    ),
  }
}

// ---------------------------------------------------------------------------
// Visibility
// ---------------------------------------------------------------------------
describe('CommandPalette', () => {
  afterEach(() => {
    cleanup()
  })

  it('is closed by default (not visible in the DOM)', () => {
    renderPalette()
    expect(screen.queryByPlaceholderText('Type a command or search...')).not.toBeInTheDocument()
  })

  // ---------------------------------------------------------------------------
  // Open / Close
  // ---------------------------------------------------------------------------
  it('opens when Cmd+K is pressed', async () => {
    const user = userEvent.setup()
    renderPalette()

    await user.keyboard('{Meta>}k{/Meta}')

    expect(screen.getByPlaceholderText('Type a command or search...')).toBeInTheDocument()
  })

  it('closes when Escape is pressed', async () => {
    const user = userEvent.setup()
    renderPalette()

    // Open
    await user.keyboard('{Meta>}k{/Meta}')
    expect(screen.getByPlaceholderText('Type a command or search...')).toBeInTheDocument()

    // Close
    await user.keyboard('{Escape}')
    expect(screen.queryByPlaceholderText('Type a command or search...')).not.toBeInTheDocument()
  })

  // ---------------------------------------------------------------------------
  // Navigation commands
  // ---------------------------------------------------------------------------
  it('shows navigation commands', async () => {
    const user = userEvent.setup()
    renderPalette()

    await user.keyboard('{Meta>}k{/Meta}')

    expect(screen.getByText('Live Feed')).toBeInTheDocument()
    expect(screen.getByText('Orderbook')).toBeInTheDocument()
    expect(screen.getByText('Replay Workbench')).toBeInTheDocument()
    expect(screen.getByText('Execution Inspector')).toBeInTheDocument()
    expect(screen.getByText('Integrity Dashboard')).toBeInTheDocument()
    expect(screen.getByText('Query Workbench')).toBeInTheDocument()
  })

  // ---------------------------------------------------------------------------
  // Filtering
  // ---------------------------------------------------------------------------
  it('filters commands when typing in the search input', async () => {
    const user = userEvent.setup()
    renderPalette()

    await user.keyboard('{Meta>}k{/Meta}')

    // All navigation items visible initially
    expect(screen.getByText('Live Feed')).toBeInTheDocument()
    expect(screen.getByText('Orderbook')).toBeInTheDocument()

    // Type a filter term — cmdk filters based on the item text content
    await user.type(screen.getByPlaceholderText('Type a command or search...'), 'Orderbook')

    // Orderbook should remain visible
    expect(screen.getByText('Orderbook')).toBeInTheDocument()

    // Other navigation items should be filtered out
    expect(screen.queryByText('Live Feed')).not.toBeInTheDocument()
    expect(screen.queryByText('Replay Workbench')).not.toBeInTheDocument()
    expect(screen.queryByText('Execution Inspector')).not.toBeInTheDocument()
    expect(screen.queryByText('Integrity Dashboard')).not.toBeInTheDocument()
    expect(screen.queryByText('Query Workbench')).not.toBeInTheDocument()
  })
})
