import { useCallback, useEffect, useState } from 'react'

export type Theme = 'dark' | 'light'
export type Density = 'compact' | 'comfortable' | 'spacious'

const THEME_KEY = 'pb-theme'
const DENSITY_KEY = 'pb-density'

function getInitialTheme(): Theme {
  const stored = localStorage.getItem(THEME_KEY)
  if (stored === 'dark' || stored === 'light') return stored
  return 'dark'
}

function getInitialDensity(): Density {
  const stored = localStorage.getItem(DENSITY_KEY)
  if (stored === 'compact' || stored === 'comfortable' || stored === 'spacious') return stored
  return 'comfortable'
}

function applyTheme(theme: Theme) {
  const root = document.documentElement
  root.classList.remove('theme-light')
  if (theme === 'light') root.classList.add('theme-light')
  localStorage.setItem(THEME_KEY, theme)
}

function applyDensity(density: Density) {
  const root = document.documentElement
  root.classList.remove('density-compact', 'density-spacious')
  if (density !== 'comfortable') root.classList.add(`density-${density}`)
  localStorage.setItem(DENSITY_KEY, density)
}

export function useTheme() {
  const [theme, setThemeState] = useState<Theme>(getInitialTheme)
  const [density, setDensityState] = useState<Density>(getInitialDensity)

  useEffect(() => {
    applyTheme(theme)
  }, [theme])

  useEffect(() => {
    applyDensity(density)
  }, [density])

  const setTheme = useCallback((t: Theme) => setThemeState(t), [])
  const toggleTheme = useCallback(
    () => setThemeState((prev) => (prev === 'dark' ? 'light' : 'dark')),
    [],
  )
  const setDensity = useCallback((d: Density) => setDensityState(d), [])

  return { theme, setTheme, toggleTheme, density, setDensity }
}
