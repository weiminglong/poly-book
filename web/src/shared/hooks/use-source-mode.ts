import { createContext, useCallback, useContext, useState } from 'react'
import type { DataSourceMode } from '../../types'

const SOURCE_KEY = 'pb-workstation-source-mode'

function getInitialSourceMode(): DataSourceMode {
  const urlMode = new URLSearchParams(window.location.search).get('source')
  if (urlMode === 'demo' || urlMode === 'api') return urlMode

  const stored = localStorage.getItem(SOURCE_KEY)
  if (stored === 'demo' || stored === 'api') return stored

  // Build-time default: the hosted GitHub Pages demo has no backend, so it
  // ships with demo as the fallback instead of a dead API connection.
  return import.meta.env.VITE_DEFAULT_SOURCE_MODE === 'demo' ? 'demo' : 'api'
}

export function useSourceMode() {
  const [sourceMode, setSourceModeState] = useState<DataSourceMode>(getInitialSourceMode)

  const setSourceMode = useCallback((mode: DataSourceMode) => {
    localStorage.setItem(SOURCE_KEY, mode)
    setSourceModeState(mode)
  }, [])

  return { sourceMode, setSourceMode }
}

// Context for sharing source mode with query hooks
type SourceModeContextValue = { sourceMode: DataSourceMode }

const SourceModeContext = createContext<SourceModeContextValue>({ sourceMode: 'api' })

export { SourceModeContext }

export function useSourceModeContext(): DataSourceMode {
  return useContext(SourceModeContext).sourceMode
}
