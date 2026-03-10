import { useEffect, useRef } from 'react'
import { useLocation } from 'react-router-dom'

/**
 * Moves focus to the page heading or main content container after
 * each route change so keyboard and screen-reader users land in
 * the right place.
 */
export function useFocusOnNavigate() {
  const { pathname } = useLocation()
  const isFirstRender = useRef(true)

  // biome-ignore lint/correctness/useExhaustiveDependencies: pathname triggers focus on route change
  useEffect(() => {
    // Skip the initial mount so the browser's default focus is preserved.
    if (isFirstRender.current) {
      isFirstRender.current = false
      return
    }

    const target =
      document.getElementById('page-heading') ?? document.getElementById('main-content')

    if (!target) return

    // Allow non-interactive elements to receive focus without
    // appearing in the regular tab order.
    if (!target.hasAttribute('tabindex')) {
      target.setAttribute('tabindex', '-1')
    }

    target.focus({ preventScroll: false })
  }, [pathname])
}
