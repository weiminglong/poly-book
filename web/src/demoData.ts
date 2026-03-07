import type {
  ActiveAssetSummary,
  FeedStatusResponse,
  LiveOrderBookSnapshot,
  ReplayFormValues,
  ReplayMode,
  ReplayReconstructionResponse,
} from './types'

const DEMO_TIMESTAMP_US = 1_741_344_480_123_000
const DEMO_ROTATION_US = 1_741_344_420_000_000

export const demoFeedStatus: FeedStatusResponse = {
  mode: 'auto_rotate',
  session_status: 'connected',
  current_session_id: 'ws-session-demo-btc-5m',
  active_asset_count: 2,
  active_assets: ['btc-5m-yes', 'btc-5m-no'],
  last_rotation_us: DEMO_ROTATION_US,
  latest_global_warning: {
    kind: 'reconnect_success',
    recv_timestamp_us: DEMO_TIMESTAMP_US - 6_000_000,
    exchange_timestamp_us: DEMO_TIMESTAMP_US - 6_250_000,
    details: 'Recovered after a transient websocket reconnect.',
  },
}

export const demoActiveAssets: ActiveAssetSummary[] = [
  {
    asset_id: 'btc-5m-yes',
    last_recv_timestamp_us: DEMO_TIMESTAMP_US,
    last_exchange_timestamp_us: DEMO_TIMESTAMP_US - 180_000,
    stale: false,
    has_book: true,
  },
  {
    asset_id: 'btc-5m-no',
    last_recv_timestamp_us: DEMO_TIMESTAMP_US - 220_000,
    last_exchange_timestamp_us: DEMO_TIMESTAMP_US - 310_000,
    stale: false,
    has_book: true,
  },
]

const demoBooks: Record<string, LiveOrderBookSnapshot> = {
  'btc-5m-yes': {
    asset_id: 'btc-5m-yes',
    sequence: 482_109,
    last_update_us: DEMO_TIMESTAMP_US,
    best_bid: { price: '0.5325', size: '158.600000' },
    best_ask: { price: '0.5337', size: '136.250000' },
    mid_price: 0.5331,
    spread: 0.0012,
    bid_depth: 6,
    ask_depth: 6,
    bids: [
      { price: '0.5325', size: '158.600000' },
      { price: '0.5320', size: '211.500000' },
      { price: '0.5314', size: '96.400000' },
      { price: '0.5308', size: '180.125000' },
      { price: '0.5299', size: '210.020000' },
      { price: '0.5292', size: '133.330000' },
    ],
    asks: [
      { price: '0.5337', size: '136.250000' },
      { price: '0.5341', size: '142.010000' },
      { price: '0.5350', size: '104.700000' },
      { price: '0.5360', size: '165.333000' },
      { price: '0.5371', size: '97.400000' },
      { price: '0.5382', size: '122.008000' },
    ],
    stale: false,
    latest_warning: {
      kind: 'sequence_gap_detected',
      recv_timestamp_us: DEMO_TIMESTAMP_US - 2_200_000,
      exchange_timestamp_us: DEMO_TIMESTAMP_US - 2_600_000,
      details: 'Gap recovered with a fresh snapshot before this view.',
    },
  },
  'btc-5m-no': {
    asset_id: 'btc-5m-no',
    sequence: 482_110,
    last_update_us: DEMO_TIMESTAMP_US - 125_000,
    best_bid: { price: '0.4660', size: '121.800000' },
    best_ask: { price: '0.4672', size: '101.400000' },
    mid_price: 0.4666,
    spread: 0.0012,
    bid_depth: 5,
    ask_depth: 5,
    bids: [
      { price: '0.4660', size: '121.800000' },
      { price: '0.4655', size: '144.200000' },
      { price: '0.4648', size: '111.125000' },
      { price: '0.4640', size: '88.700000' },
      { price: '0.4633', size: '150.010000' },
    ],
    asks: [
      { price: '0.4672', size: '101.400000' },
      { price: '0.4680', size: '132.900000' },
      { price: '0.4687', size: '99.200000' },
      { price: '0.4694', size: '110.015000' },
      { price: '0.4702', size: '140.100000' },
    ],
    stale: false,
    latest_warning: null,
  },
}

const demoReplayBooks: Record<string, ReplayReconstructionResponse> = {
  'btc-5m-yes:recv_time': {
    asset_id: 'btc-5m-yes',
    mode: 'recv_time',
    used_checkpoint: true,
    sequence: 481_902,
    last_update_us: DEMO_TIMESTAMP_US - 45_000_000,
    best_bid: { price: '0.5290', size: '147.100000' },
    best_ask: { price: '0.5305', size: '109.800000' },
    mid_price: 0.52975,
    spread: 0.0015,
    bid_depth: 5,
    ask_depth: 5,
    bids: [
      { price: '0.5290', size: '147.100000' },
      { price: '0.5284', size: '131.600000' },
      { price: '0.5276', size: '180.110000' },
      { price: '0.5268', size: '84.700000' },
      { price: '0.5261', size: '90.300000' },
    ],
    asks: [
      { price: '0.5305', size: '109.800000' },
      { price: '0.5312', size: '99.400000' },
      { price: '0.5320', size: '121.300000' },
      { price: '0.5329', size: '88.250000' },
      { price: '0.5335', size: '133.030000' },
    ],
    continuity_events: [
      {
        kind: 'checkpoint_loaded',
        recv_timestamp_us: DEMO_TIMESTAMP_US - 50_000_000,
        exchange_timestamp_us: DEMO_TIMESTAMP_US - 50_500_000,
        details: 'Replay started from a stored checkpoint.',
      },
      {
        kind: 'reconnect_boundary',
        recv_timestamp_us: DEMO_TIMESTAMP_US - 46_000_000,
        exchange_timestamp_us: DEMO_TIMESTAMP_US - 46_400_000,
        details: 'Events crossing this boundary are best-effort.',
      },
    ],
  },
  'btc-5m-yes:exchange_time': {
    asset_id: 'btc-5m-yes',
    mode: 'exchange_time',
    used_checkpoint: false,
    sequence: 481_888,
    last_update_us: DEMO_TIMESTAMP_US - 45_300_000,
    best_bid: { price: '0.5287', size: '142.200000' },
    best_ask: { price: '0.5301', size: '115.450000' },
    mid_price: 0.5294,
    spread: 0.0014,
    bid_depth: 4,
    ask_depth: 4,
    bids: [
      { price: '0.5287', size: '142.200000' },
      { price: '0.5280', size: '120.100000' },
      { price: '0.5274', size: '87.330000' },
      { price: '0.5265', size: '143.800000' },
    ],
    asks: [
      { price: '0.5301', size: '115.450000' },
      { price: '0.5308', size: '107.700000' },
      { price: '0.5316', size: '111.020000' },
      { price: '0.5322', size: '140.500000' },
    ],
    continuity_events: [
      {
        kind: 'sequence_gap_detected',
        recv_timestamp_us: DEMO_TIMESTAMP_US - 47_000_000,
        exchange_timestamp_us: DEMO_TIMESTAMP_US - 47_200_000,
        details: 'One gap remains unresolved in exchange-time ordering.',
      },
    ],
  },
}

export const demoReplayDefaults: ReplayFormValues = {
  assetId: 'btc-5m-yes',
  atUs: String(DEMO_TIMESTAMP_US - 45_000_000),
  mode: 'recv_time',
  depth: 5,
}

export const demoTimestampHint = DEMO_TIMESTAMP_US

export function getDemoSnapshot(assetId: string, depth: number): LiveOrderBookSnapshot {
  const book = demoBooks[assetId]
  if (!book) {
    throw new Error(`asset not active: ${assetId}`)
  }

  return {
    ...book,
    bids: book.bids.slice(0, depth),
    asks: book.asks.slice(0, depth),
  }
}

export function getDemoReplay(assetId: string, mode: ReplayMode, depth: number): ReplayReconstructionResponse {
  const replay = demoReplayBooks[`${assetId}:${mode}`]
  if (!replay) {
    throw new Error(`no demo replay found for ${assetId} in ${mode}`)
  }

  return {
    ...replay,
    bids: replay.bids.slice(0, depth),
    asks: replay.asks.slice(0, depth),
  }
}
