import { describe, expect, it } from 'vitest'
import {
  formatClientTimestamp,
  formatDurationUs,
  formatIntervalMs,
  formatLevel,
  formatNumber,
  formatPrice,
  formatSize,
  formatTimestamp,
  titleCase,
} from '../formatters'

describe('formatTimestamp', () => {
  it('returns "---" for null', () => {
    expect(formatTimestamp(null)).toBe('---')
  })

  it('returns "---" for undefined', () => {
    expect(formatTimestamp(undefined)).toBe('---')
  })

  it('returns "---" for zero', () => {
    expect(formatTimestamp(0)).toBe('---')
  })

  it('converts microsecond timestamp to locale string', () => {
    const us = 1_741_344_480_123_000
    const result = formatTimestamp(us)
    // Should produce a non-placeholder string
    expect(result).not.toBe('---')
    expect(result.length).toBeGreaterThan(0)
  })
})

describe('formatClientTimestamp', () => {
  it('returns "---" for null', () => {
    expect(formatClientTimestamp(null)).toBe('---')
  })

  it('returns "---" for undefined', () => {
    expect(formatClientTimestamp(undefined)).toBe('---')
  })

  it('returns "---" for zero', () => {
    expect(formatClientTimestamp(0)).toBe('---')
  })

  it('converts millisecond timestamp to locale string', () => {
    const ms = 1_741_344_480_123
    const result = formatClientTimestamp(ms)
    expect(result).not.toBe('---')
    expect(result.length).toBeGreaterThan(0)
  })
})

describe('formatNumber', () => {
  it('returns "---" for null', () => {
    expect(formatNumber(null)).toBe('---')
  })

  it('returns "---" for undefined', () => {
    expect(formatNumber(undefined)).toBe('---')
  })

  it('returns "---" for NaN', () => {
    expect(formatNumber(Number.NaN)).toBe('---')
  })

  it('formats integer', () => {
    expect(formatNumber(1234)).toBe('1,234')
  })

  it('formats decimal with default 4 digits', () => {
    const result = formatNumber(0.53315)
    expect(result).toContain('0.533')
  })

  it('respects custom digits parameter', () => {
    const result = formatNumber(0.123456789, 2)
    expect(result).toBe('0.12')
  })

  it('formats zero correctly', () => {
    expect(formatNumber(0)).toBe('0')
  })

  it('formats negative numbers', () => {
    const result = formatNumber(-42.5)
    expect(result).toContain('42.5')
  })
})

describe('formatPrice', () => {
  it('returns "---" for null', () => {
    expect(formatPrice(null)).toBe('---')
  })

  it('returns "---" for undefined', () => {
    expect(formatPrice(undefined)).toBe('---')
  })

  it('returns "---" for empty string', () => {
    expect(formatPrice('')).toBe('---')
  })

  it('formats price string to 4 decimal places', () => {
    expect(formatPrice('0.5325')).toBe('0.5325')
  })

  it('pads short decimals', () => {
    expect(formatPrice('0.53')).toBe('0.5300')
  })

  it('truncates long decimals', () => {
    expect(formatPrice('0.532567')).toBe('0.5326')
  })
})

describe('formatSize', () => {
  it('returns "---" for null', () => {
    expect(formatSize(null)).toBe('---')
  })

  it('returns "---" for undefined', () => {
    expect(formatSize(undefined)).toBe('---')
  })

  it('returns "---" for empty string', () => {
    expect(formatSize('')).toBe('---')
  })

  it('formats size string with up to 6 decimal places', () => {
    const result = formatSize('158.600000')
    expect(result).toContain('158.6')
  })

  it('formats integer-like size without trailing zeros', () => {
    const result = formatSize('100.000000')
    expect(result).toBe('100')
  })
})

describe('formatLevel', () => {
  it('returns "---" for null', () => {
    expect(formatLevel(null)).toBe('---')
  })

  it('formats price @ size', () => {
    const result = formatLevel({ price: '0.5325', size: '158.600000' })
    expect(result).toBe('0.5325 @ 158.6')
  })
})

describe('titleCase', () => {
  it('converts snake_case to Title Case', () => {
    expect(titleCase('auto_rotate')).toBe('Auto Rotate')
  })

  it('handles single word', () => {
    expect(titleCase('connected')).toBe('Connected')
  })

  it('handles multiple underscores', () => {
    expect(titleCase('a_b_c')).toBe('A B C')
  })

  it('handles empty string', () => {
    expect(titleCase('')).toBe('')
  })
})

describe('formatIntervalMs', () => {
  it('formats sub-second interval with one decimal', () => {
    expect(formatIntervalMs(500)).toBe('0.5s')
  })

  it('formats one second exactly', () => {
    expect(formatIntervalMs(1000)).toBe('1s')
  })

  it('formats multi-second interval without decimals', () => {
    expect(formatIntervalMs(5000)).toBe('5s')
  })

  it('formats fractional seconds above 1s without decimals', () => {
    expect(formatIntervalMs(1500)).toBe('2s')
  })
})

describe('formatDurationUs', () => {
  it('formats sub-millisecond in microseconds', () => {
    expect(formatDurationUs(500)).toBe('500µs')
  })

  it('formats boundary at 999µs', () => {
    expect(formatDurationUs(999)).toBe('999µs')
  })

  it('formats milliseconds with one decimal', () => {
    expect(formatDurationUs(1_500)).toBe('1.5ms')
  })

  it('formats seconds with two decimals', () => {
    expect(formatDurationUs(1_500_000)).toBe('1.50s')
  })

  it('formats exact 1ms', () => {
    expect(formatDurationUs(1_000)).toBe('1.0ms')
  })

  it('formats exact 1s', () => {
    expect(formatDurationUs(1_000_000)).toBe('1.00s')
  })
})
