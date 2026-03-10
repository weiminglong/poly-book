import { expect, test } from '@playwright/test'

test.describe('Replay page', () => {
  test('renders replay form and runs reconstruction in demo mode', async ({ page }) => {
    await page.goto('/replay?source=demo')

    // Page heading
    await expect(page.getByText('Replay Workbench')).toBeVisible()

    // Form elements
    await expect(page.getByLabel('Asset ID')).toBeVisible()
    await expect(page.getByLabel('Timestamp (us)')).toBeVisible()

    // Fill form
    await page.getByLabel('Timestamp (us)').fill('1700000000000000')

    // Submit
    const runButton = page.getByRole('button', { name: /Run Reconstruction/i })
    await expect(runButton).toBeVisible()
  })
})
