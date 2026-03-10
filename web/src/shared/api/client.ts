import type { ZodSchema } from 'zod'
import type { RequestOptions } from '../../types'

const defaultApiBaseUrl = import.meta.env.VITE_API_BASE_URL ?? ''
const REQUEST_TIMEOUT_MS = 4_000

export function buildUrl(
  pathPrefix: string,
  path: string,
  params?: Record<string, string>,
): string {
  const base = pathPrefix.endsWith('/') ? pathPrefix.slice(0, -1) : pathPrefix
  const url = `${base}${path}`
  if (!params) return url
  const query = new URLSearchParams(params)
  return `${url}?${query.toString()}`
}

export async function fetchAndValidate<T>(
  schema: ZodSchema<T>,
  url: string,
  options?: RequestOptions,
): Promise<T> {
  const timeoutController = new AbortController()
  let timedOut = false
  const timeoutId = window.setTimeout(() => {
    timedOut = true
    timeoutController.abort()
  }, REQUEST_TIMEOUT_MS)

  const abortHandler = () => timeoutController.abort()
  options?.signal?.addEventListener('abort', abortHandler, { once: true })

  try {
    const response = await fetch(url, { signal: timeoutController.signal })

    if (!response.ok) {
      let message = `Request failed with status ${response.status}`
      try {
        const body = await response.json()
        if (typeof body?.error === 'string') {
          message = body.error
        }
      } catch {
        // fall back to HTTP status message
      }
      throw new Error(message)
    }

    const json: unknown = await response.json()
    return schema.parse(json)
  } catch (error) {
    if (timedOut) {
      throw new Error(`Request timed out after ${REQUEST_TIMEOUT_MS}ms`)
    }
    throw error
  } finally {
    window.clearTimeout(timeoutId)
    options?.signal?.removeEventListener('abort', abortHandler)
  }
}

export async function postAndValidate<T>(
  schema: ZodSchema<T>,
  url: string,
  body: unknown,
  options?: RequestOptions,
): Promise<T> {
  const timeoutController = new AbortController()
  let timedOut = false
  const timeoutId = window.setTimeout(() => {
    timedOut = true
    timeoutController.abort()
  }, REQUEST_TIMEOUT_MS)

  const abortHandler = () => timeoutController.abort()
  options?.signal?.addEventListener('abort', abortHandler, { once: true })

  try {
    const response = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      signal: timeoutController.signal,
    })

    if (!response.ok) {
      let message = `Request failed with status ${response.status}`
      try {
        const json = await response.json()
        if (typeof json?.error === 'string') {
          message = json.error
        }
      } catch {
        // fall back
      }
      throw new Error(message)
    }

    const json: unknown = await response.json()
    return schema.parse(json)
  } catch (error) {
    if (timedOut) {
      throw new Error(`Request timed out after ${REQUEST_TIMEOUT_MS}ms`)
    }
    throw error
  } finally {
    window.clearTimeout(timeoutId)
    options?.signal?.removeEventListener('abort', abortHandler)
  }
}

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException
    ? error.name === 'AbortError'
    : error instanceof Error && error.name === 'AbortError'
}

export function getApiBaseUrl(): string {
  return defaultApiBaseUrl
}
