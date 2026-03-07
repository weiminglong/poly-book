import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)

if (import.meta.env.PROD && typeof PerformanceObserver !== 'undefined') {
  try {
    const logEntry = (name: string, value: number) =>
      console.log(`[perf] ${name}: ${Math.round(value)}ms`)

    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        if (entry.entryType === 'largest-contentful-paint') {
          logEntry('LCP', entry.startTime)
        }
      }
    }).observe({ type: 'largest-contentful-paint', buffered: true })

    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        if (entry.entryType === 'first-input') {
          logEntry('FID', (entry as PerformanceEventTiming).processingStart - entry.startTime)
        }
      }
    }).observe({ type: 'first-input', buffered: true })

    new PerformanceObserver((list) => {
      let clsValue = 0
      for (const entry of list.getEntries()) {
        if (!(entry as LayoutShift).hadRecentInput) {
          clsValue += (entry as LayoutShift).value
        }
      }
      if (clsValue > 0) {
        console.log(`[perf] CLS: ${clsValue.toFixed(4)}`)
      }
    }).observe({ type: 'layout-shift', buffered: true })
  } catch {
    // PerformanceObserver not supported; skip silently.
  }
}

interface LayoutShift extends PerformanceEntry {
  hadRecentInput: boolean
  value: number
}
