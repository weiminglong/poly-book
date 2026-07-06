import { memo, useCallback, useEffect, useMemo, useRef } from 'react'
import type { PriceLevelView } from '../../../types'

interface DepthChartProps {
  bids: PriceLevelView[]
  asks: PriceLevelView[]
}

interface CumLevel {
  price: number
  cumSize: number
}

function computeCumulative(levels: PriceLevelView[]): CumLevel[] {
  let cum = 0
  return levels.map((l) => {
    cum += Number.parseFloat(l.size)
    return { price: Number.parseFloat(l.price), cumSize: cum }
  })
}

export const DepthChart = memo(function DepthChart({ bids, asks }: DepthChartProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  // Cached CSS-pixel size + dpr, updated only by the ResizeObserver — so the hot
  // draw path (which runs on every WS update) does no getBoundingClientRect and
  // never writes canvas.width/height, avoiding read/write layout thrash.
  // canvas.width is reset only when the size actually changes.
  const sizeRef = useRef({ w: 0, h: 0, dpr: 1 })

  // Cumulative depth is derived from the levels, so memoize it instead of
  // recomputing on every render/draw.
  const cumBids = useMemo(() => computeCumulative(bids), [bids])
  const cumAsks = useMemo(() => computeCumulative(asks), [asks])

  const draw = useCallback(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const ctx = canvas.getContext('2d')
    if (!ctx) return
    const { w, h, dpr } = sizeRef.current
    if (w === 0 || h === 0) return

    // setTransform (not scale) so the dpr transform is set, not multiplied — the
    // backing store is no longer reset every draw, so a cumulative scale() would
    // otherwise compound.
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    ctx.clearRect(0, 0, w, h)

    if (cumBids.length === 0 && cumAsks.length === 0) return

    // Single pass for min/max instead of Math.min(...spread)/Math.max(...spread),
    // which allocate intermediate arrays and blow the call stack on deep books.
    let minPrice = Number.POSITIVE_INFINITY
    let maxPrice = Number.NEGATIVE_INFINITY
    let maxSize = 1
    for (const p of cumBids) {
      if (p.price < minPrice) minPrice = p.price
      if (p.price > maxPrice) maxPrice = p.price
      if (p.cumSize > maxSize) maxSize = p.cumSize
    }
    for (const p of cumAsks) {
      if (p.price < minPrice) minPrice = p.price
      if (p.price > maxPrice) maxPrice = p.price
      if (p.cumSize > maxSize) maxSize = p.cumSize
    }
    const priceRange = maxPrice - minPrice || 1

    const scaleX = (price: number) => ((price - minPrice) / priceRange) * w
    const scaleY = (size: number) => h - (size / maxSize) * (h - 20)

    // Draw bid area (green)
    if (cumBids.length > 0) {
      ctx.beginPath()
      ctx.moveTo(scaleX(cumBids[0].price), h)
      for (const p of cumBids) {
        ctx.lineTo(scaleX(p.price), scaleY(p.cumSize))
      }
      ctx.lineTo(scaleX(cumBids[cumBids.length - 1].price), h)
      ctx.closePath()
      ctx.fillStyle = 'rgba(34, 197, 94, 0.2)'
      ctx.fill()

      // Line
      ctx.beginPath()
      for (let i = 0; i < cumBids.length; i++) {
        const fn = i === 0 ? ctx.moveTo : ctx.lineTo
        fn.call(ctx, scaleX(cumBids[i].price), scaleY(cumBids[i].cumSize))
      }
      ctx.strokeStyle = '#22c55e'
      ctx.lineWidth = 2
      ctx.stroke()
    }

    // Draw ask area (red)
    if (cumAsks.length > 0) {
      ctx.beginPath()
      ctx.moveTo(scaleX(cumAsks[0].price), h)
      for (const p of cumAsks) {
        ctx.lineTo(scaleX(p.price), scaleY(p.cumSize))
      }
      ctx.lineTo(scaleX(cumAsks[cumAsks.length - 1].price), h)
      ctx.closePath()
      ctx.fillStyle = 'rgba(239, 68, 68, 0.2)'
      ctx.fill()

      ctx.beginPath()
      for (let i = 0; i < cumAsks.length; i++) {
        const fn = i === 0 ? ctx.moveTo : ctx.lineTo
        fn.call(ctx, scaleX(cumAsks[i].price), scaleY(cumAsks[i].cumSize))
      }
      ctx.strokeStyle = '#ef4444'
      ctx.lineWidth = 2
      ctx.stroke()
    }

    // Mid price line
    if (cumBids.length > 0 && cumAsks.length > 0) {
      const midPrice = (cumBids[0].price + cumAsks[0].price) / 2
      const midX = scaleX(midPrice)
      ctx.beginPath()
      ctx.moveTo(midX, 0)
      ctx.lineTo(midX, h)
      ctx.strokeStyle = 'rgba(148, 163, 184, 0.4)'
      ctx.lineWidth = 1
      ctx.setLineDash([4, 4])
      ctx.stroke()
      ctx.setLineDash([])

      // Label
      ctx.fillStyle = '#94a3b8'
      ctx.font = '11px Inter, sans-serif'
      ctx.textAlign = 'center'
      ctx.fillText(`Mid ${midPrice.toFixed(4)}`, midX, 14)
    }
  }, [cumBids, cumAsks])

  // Keep a ref to the latest draw so the ResizeObserver effect can stay stable
  // (set up once) rather than re-subscribing whenever the data changes.
  const drawRef = useRef(draw)
  drawRef.current = draw

  // Size tracking via ResizeObserver so the chart re-fits its container, not just
  // the window. Measures + resizes the backing store + redraws.
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const measure = () => {
      const rect = canvas.getBoundingClientRect()
      const dpr = window.devicePixelRatio || 1
      sizeRef.current = { w: rect.width, h: rect.height, dpr }
      const pxW = Math.round(rect.width * dpr)
      const pxH = Math.round(rect.height * dpr)
      if (canvas.width !== pxW || canvas.height !== pxH) {
        canvas.width = pxW
        canvas.height = pxH
      }
      drawRef.current()
    }
    measure()
    // Prefer ResizeObserver (re-fits the container, not just the window), but
    // fall back to the window resize listener where it is unavailable (jsdom in
    // tests, older browsers) so the chart still resizes and nothing throws.
    if (typeof ResizeObserver !== 'undefined') {
      const observer = new ResizeObserver(measure)
      observer.observe(canvas)
      return () => observer.disconnect()
    }
    window.addEventListener('resize', measure)
    return () => window.removeEventListener('resize', measure)
  }, [])

  // Redraw on data change (uses the cached size; no canvas resize / no thrash).
  useEffect(() => {
    draw()
  }, [draw])

  return <canvas ref={canvasRef} className="h-[250px] w-full" style={{ display: 'block' }} />
})
