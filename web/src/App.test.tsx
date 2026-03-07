import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { TestableApp } from './App'

describe('workstation SPA', () => {
  it('renders the live feed route with seeded demo data', async () => {
    render(<TestableApp initialEntries={['/live-feed']} sourceMode="demo" />)

    expect(await screen.findByText('ws-session-demo-btc-5m')).toBeInTheDocument()
    expect(await screen.findByRole('heading', { name: /order book · btc-5m-yes/i })).toBeInTheDocument()
    expect(screen.getByText('Sequence')).toBeInTheDocument()
  })

  it('renders the replay lab route with a seeded reconstruction result', async () => {
    render(<TestableApp initialEntries={['/live-feed']} sourceMode="demo" />)

    await userEvent.click(screen.getByRole('link', { name: /replay lab/i }))
    await screen.findByRole('button', { name: /load demo query/i })
    await userEvent.click(screen.getByRole('button', { name: /load demo query/i }))
    await userEvent.click(screen.getByRole('button', { name: /run reconstruction/i }))
    expect(await screen.findByText('Used checkpoint')).toBeInTheDocument()
    expect(await screen.findByText(/Replay started from a stored checkpoint\./i)).toBeInTheDocument()
    expect(screen.getByText('Continuity events')).toBeInTheDocument()
  })
})
