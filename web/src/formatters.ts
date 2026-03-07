import type { PriceLevelView } from './types'

const microsecondMultiplier = 1000

export function formatTimestamp(timestampUs: number | null | undefined): string {
  if (!timestampUs) {
    return '—'
  }

  return new Date(timestampUs / microsecondMultiplier).toLocaleString()
}

export function formatClientTimestamp(timestampMs: number | null | undefined): string {
  if (!timestampMs) {
    return '—'
  }

  return new Date(timestampMs).toLocaleString()
}

export function formatNumber(value: number | null | undefined, digits = 4): string {
  if (value === null || value === undefined || Number.isNaN(value)) {
    return '—'
  }

  return value.toLocaleString(undefined, {
    minimumFractionDigits: 0,
    maximumFractionDigits: digits,
  })
}

export function formatPrice(value: string | null | undefined): string {
  if (!value) {
    return '—'
  }

  return Number.parseFloat(value).toFixed(4)
}

export function formatSize(value: string | null | undefined): string {
  if (!value) {
    return '—'
  }

  return Number.parseFloat(value).toLocaleString(undefined, {
    minimumFractionDigits: 0,
    maximumFractionDigits: 6,
  })
}

export function formatLevel(level: PriceLevelView | null): string {
  if (!level) {
    return '—'
  }

  return `${formatPrice(level.price)} @ ${formatSize(level.size)}`
}

export function titleCase(value: string): string {
  return value
    .split('_')
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ')
}

export function formatIntervalMs(intervalMs: number): string {
  return `${(intervalMs / 1000).toFixed(intervalMs < 1000 ? 1 : 0)}s`
}
