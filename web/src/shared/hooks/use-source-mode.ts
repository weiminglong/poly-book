import { createContext, useCallback, useContext, useState } from 'react'
import type { DataSourceMode } from '../../types'

const SOURCE_KEY = 'pb-workstation-source-mode'

function getInitialSourceMode(): DataSourceMode {
  const urlMode = new URLSearchParams(window.location.search).get('source')
  if (urlMode === 'demo' || urlMode === 'api') return urlMode

  const stored = localStorage.getItem(SOURCE_KEY)
  if (stored === 'demo' || stored === 'api') return stored

  return 'api'
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
