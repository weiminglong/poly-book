import type { BookUpdateMessage, PriceLevelView } from '../../types'

/**
 * Client-side synthetic market for demo mode.
 *
 * Seeds from the demo fixture book, then random-walks it: per-tick size
 * jitter on a few levels, occasional one-tick mid shifts (rebuilding the
 * ladder around the new mid), and a bounded [0.05, 0.95] price band so a
 * Polymarket-style binary-outcome book never walks off its probability
 * range. Emits `BookUpdateMessage`-shaped frames — the same shape the real
 * WebSocket broadcast produces — so the page renders identically in demo
 * and live mode.
 */

const TICK = 0.0001
const PRICE_DECIMALS = 4
const SIZE_DECIMALS = 6
const MIN_MID = 0.05
const MAX_MID = 0.95
const INTERVAL_MS = 140
/** Probability per tick that the mid shifts by one price tick. */
const MID_SHIFT_P = 0.18
/** Probability per tick that the spread widens/narrows by one tick. */
const SPREAD_CHANGE_P = 0.06
const MIN_SPREAD_TICKS = 8
const MAX_SPREAD_TICKS = 24
const LEVELS_PER_SIDE = 10

interface Level {
  price: number
  size: number
}

interface SimState {
  bestBid: number
  bestAsk: number
  bids: Level[]
  asks: Level[]
  sequence: number
}

function roundPrice(p: number): number {
  return Number(p.toFixed(PRICE_DECIMALS))
}

function levelSize(base: number): number {
  return Number((base * (0.6 + Math.random() * 0.9)).toFixed(SIZE_DECIMALS))
}

/** Rebuild one side's ladder outward from its best price. */
function buildSide(best: number, direction: -1 | 1, prev: Level[]): Level[] {
  const side: Level[] = []
  let price = best
  for (let i = 0; i < LEVELS_PER_SIDE; i++) {
    // Reuse the previous size at an unchanged price so the ladder does not
    // visually re-roll every level on a one-tick mid shift.
    const kept = prev.find((l) => l.price === roundPrice(price))
    side.push({
      price: roundPrice(price),
      size: kept ? kept.size : levelSize(90 + i * 14),
    })
    // Gaps grow away from the touch, like a real book.
    price += direction * TICK * (1 + Math.floor(Math.random() * 3) + Math.floor(i / 3))
  }
  return side
}

function seedState(seed: {
  bids: PriceLevelView[]
  asks: PriceLevelView[]
  sequence: number
}): SimState {
  const bestBid = Number(seed.bids[0]?.price ?? '0.5000')
  const bestAsk = Number(seed.asks[0]?.price ?? '0.5012')
  return {
    bestBid,
    bestAsk,
    bids: buildSide(
      bestBid,
      -1,
      seed.bids.map((l) => ({ price: Number(l.price), size: Number(l.size) })),
    ),
    asks: buildSide(
      bestAsk,
      1,
      seed.asks.map((l) => ({ price: Number(l.price), size: Number(l.size) })),
    ),
    sequence: seed.sequence,
  }
}

function step(state: SimState): void {
  // Size jitter on 1-3 random levels per side.
  for (const side of [state.bids, state.asks]) {
    const touches = 1 + Math.floor(Math.random() * 3)
    for (let i = 0; i < touches; i++) {
      const idx = Math.floor(Math.random() * side.length)
      const level = side[idx]
      if (!level) continue
      const jittered = level.size * (0.82 + Math.random() * 0.36)
      level.size = Number(Math.max(5, jittered).toFixed(SIZE_DECIMALS))
    }
  }

  const spreadTicks = Math.round((state.bestAsk - state.bestBid) / TICK)

  if (Math.random() < SPREAD_CHANGE_P) {
    const widen =
      spreadTicks <= MIN_SPREAD_TICKS
        ? 1
        : spreadTicks >= MAX_SPREAD_TICKS
          ? -1
          : Math.random() < 0.5
            ? 1
            : -1
    state.bestAsk = roundPrice(state.bestAsk + widen * TICK)
    state.asks = buildSide(state.bestAsk, 1, state.asks)
  }

  if (Math.random() < MID_SHIFT_P) {
    const mid = (state.bestBid + state.bestAsk) / 2
    const drift = mid < MIN_MID ? 1 : mid > MAX_MID ? -1 : Math.random() < 0.5 ? -1 : 1
    state.bestBid = roundPrice(state.bestBid + drift * TICK)
    state.bestAsk = roundPrice(state.bestAsk + drift * TICK)
    state.bids = buildSide(state.bestBid, -1, state.bids)
    state.asks = buildSide(state.bestAsk, 1, state.asks)
  }

  state.sequence += 1
}

function toMessage(assetId: string, state: SimState): BookUpdateMessage {
  const mid = (state.bestBid + state.bestAsk) / 2
  return {
    asset_id: assetId,
    sequence: state.sequence,
    last_update_us: Date.now() * 1000,
    bid_depth: state.bids.length,
    ask_depth: state.asks.length,
    bids: state.bids.map((l) => ({
      price: l.price.toFixed(PRICE_DECIMALS),
      size: l.size.toFixed(SIZE_DECIMALS),
    })),
    asks: state.asks.map((l) => ({
      price: l.price.toFixed(PRICE_DECIMALS),
      size: l.size.toFixed(SIZE_DECIMALS),
    })),
    mid_price: roundPrice(mid),
    spread: roundPrice(state.bestAsk - state.bestBid),
  }
}

/**
 * Start the simulated stream for one asset. Returns a stop function.
 * Seeding is async (fixtures are lazy-loaded); if stopped before the seed
 * resolves, no tick ever fires.
 */
export function startDemoOrderBookStream(
  assetId: string,
  onMessage: (msg: BookUpdateMessage) => void,
): () => void {
  let timer: ReturnType<typeof setInterval> | null = null
  let stopped = false

  void (async () => {
    const { getDemoSnapshot } = await import('./demo-fixtures')
    if (stopped) return
    let seed: { bids: PriceLevelView[]; asks: PriceLevelView[]; sequence: number }
    try {
      seed = getDemoSnapshot(assetId, LEVELS_PER_SIDE)
    } catch {
      // Unknown asset id: synthesize around 0.50 rather than dead-ending.
      seed = { bids: [], asks: [], sequence: 1 }
    }
    const state = seedState(seed)
    onMessage(toMessage(assetId, state))
    timer = setInterval(() => {
      step(state)
      onMessage(toMessage(assetId, state))
    }, INTERVAL_MS)
  })()

  return () => {
    stopped = true
    if (timer) clearInterval(timer)
  }
}
