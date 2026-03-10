import { expect, test } from '@playwright/test'

test.describe('Live Feed page', () => {
  test('renders demo data and allows asset selection', async ({ page }) => {
    await page.goto('/live-feed?source=demo')

    // Page heading
    await expect(page.getByText('Live Feed')).toBeVisible()

    // Feed status metrics render
    await expect(page.getByText('Feed mode')).toBeVisible()
    await expect(page.getByText('Active assets')).toBeVisible()

    // Asset list renders
    await expect(page.getByText('btc-5m-yes')).toBeVisible()
    await expect(page.getByText('btc-5m-no')).toBeVisible()

    // Click an asset to select it
    const assetButton = page.getByRole('button', { name: /btc-5m-no/ })
    await assetButton.click()

    // Quick view header should update
    await expect(page.getByText('Quick View')).toBeVisible()
  })
})
