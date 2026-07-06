import { expect, test } from '@playwright/test'

test.describe('Orderbook page in demo mode', () => {
  test('streams a moving simulated book under a Demo badge', async ({ page }) => {
    await page.goto('/orderbook?source=demo')

    // The demo stream must present as intentional, not as a degraded
    // connection: teal badge, no reconnect/fallback noise.
    await expect(page.getByText('Demo stream')).toBeVisible()
    await expect(page.getByText('Reconnecting')).not.toBeVisible()
    await expect(page.getByText('HTTP Fallback')).not.toBeVisible()

    await expect(page.getByText('Simulated stream (demo)')).toBeVisible()

    // The book must MOVE: the simulator bumps the sequence a few times per
    // second, so two reads of the metric card cannot stay equal.
    const sequenceCard = page
      .locator('div')
      .filter({ hasText: /^Sequence\d+$/ })
      .first()
    await expect(sequenceCard).toBeVisible()
    const first = await sequenceCard.textContent()
    await expect(async () => {
      const second = await sequenceCard.textContent()
      expect(second).not.toBe(first)
    }).toPass({ timeout: 5_000 })
  })

  test('query page returns fixture results instead of erroring', async ({ page }) => {
    await page.goto('/query?source=demo')

    // The editor is pre-seeded, so Run works on the first click.
    await page.getByRole('button', { name: /run/i }).click()
    await expect(page.getByText('btc-5m-yes').first()).toBeVisible()
    await expect(page.getByText(/failed/i)).not.toBeVisible()
  })

  test('replay form is prefilled and reconstructs on first click', async ({ page }) => {
    await page.goto('/replay?source=demo')

    // The timestamp field must not dead-end empty in demo mode.
    const tsInput = page.getByLabel('Timestamp (us)')
    await expect(tsInput).not.toHaveValue('', { timeout: 5_000 })

    await page.getByRole('button', { name: /run reconstruction/i }).click()
    await expect(page.getByRole('heading', { name: 'Replay Result' })).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Bids' })).toBeVisible()
  })
})
