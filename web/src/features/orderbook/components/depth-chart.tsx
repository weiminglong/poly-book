import { memo, useCallback, useEffect, useRef } from 'react'
import type { PriceLevelView } from '../../../types'

interface DepthChartProps {
  bids: PriceLevelView[]
  asks: PriceLevelView[]
}

function computeCumulative(levels: PriceLevelView[]): { price: number; cumSize: number }[] {
  let cum = 0
  return levels.map((l) => {
    cum += Number.parseFloat(l.size)
    return { price: Number.parseFloat(l.price), cumSize: cum }
  })
}

export const DepthChart = memo(function DepthChart({ bids, asks }: DepthChartProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)

  const draw = useCallback(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const ctx = canvas.getContext('2d')
    if (!ctx) return

    const dpr = window.devicePixelRatio || 1
    const rect = canvas.getBoundingClientRect()
    canvas.width = rect.width * dpr
    canvas.height = rect.height * dpr
    ctx.scale(dpr, dpr)
    const w = rect.width
    const h = rect.height

    ctx.clearRect(0, 0, w, h)

    const cumBids = computeCumulative(bids)
    const cumAsks = computeCumulative(asks)

    if (cumBids.length === 0 && cumAsks.length === 0) return

    const allPrices = [...cumBids.map((p) => p.price), ...cumAsks.map((p) => p.price)]
    const allSizes = [...cumBids.map((p) => p.cumSize), ...cumAsks.map((p) => p.cumSize)]
    const minPrice = Math.min(...allPrices)
    const maxPrice = Math.max(...allPrices)
    const maxSize = Math.max(...allSizes, 1)
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
  }, [bids, asks])

  useEffect(() => {
    draw()
    const handleResize = () => draw()
    window.addEventListener('resize', handleResize)
    return () => window.removeEventListener('resize', handleResize)
  }, [draw])

  return <canvas ref={canvasRef} className="h-[250px] w-full" style={{ display: 'block' }} />
})
