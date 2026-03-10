## ADDED Requirements

### Requirement: Ops dashboard page
The Live Feed page SHALL be redesigned as an ops-style dashboard. It SHALL display: a connection status hero card with session ID and uptime, a feed health panel showing mode/status/rotation, an active asset grid with per-asset staleness indicators and last-seen timestamps, and a transport indicator showing whether data arrives via WebSocket or HTTP polling. The dashboard SHALL auto-refresh via TanStack Query's `refetchInterval`.

#### Scenario: All systems healthy
- **WHEN** the feed is connected and all assets are fresh
- **THEN** the connection hero card shows a green "Connected" badge, the asset grid shows all assets with "Fresh" status, and the transport indicator shows the active transport (WebSocket or HTTP polling)

#### Scenario: Asset goes stale
- **WHEN** an asset's `stale` flag is `true` in the API response
- **THEN** the asset card in the grid displays a yellow "Stale" warning badge and the last-seen timestamp is visually highlighted

#### Scenario: WebSocket fallback
- **WHEN** the WebSocket connection fails after 8 retries
- **THEN** the transport indicator changes from "WebSocket" to "HTTP Polling (fallback)" and the asset data continues to update via adaptive HTTP polling

### Requirement: Orderbook viewer page
The application SHALL have a dedicated Orderbook page (separate from Live Feed) that provides an institutional-grade orderbook visualization. It SHALL include: a price ladder showing bid/ask levels with horizontal size bars proportional to the largest size at each level, a depth chart showing cumulative bid/ask size as an area chart, and configurable depth levels (5, 10, 25, 50, 100, 200).

#### Scenario: Price ladder rendering
- **WHEN** an asset is selected and orderbook data is available
- **THEN** the price ladder displays bid levels on the left (green) and ask levels on the right (red), each with a horizontal bar whose width is proportional to the size relative to the maximum size across all visible levels

#### Scenario: Depth chart
- **WHEN** orderbook data is loaded for an asset
- **THEN** the depth chart shows cumulative bid size (left, green area) and cumulative ask size (right, red area) plotted against price, with the mid-price marked by a vertical line

#### Scenario: Depth level change
- **WHEN** the user changes the depth selector from 10 to 50
- **THEN** the price ladder and depth chart update to show 50 levels of bids and asks
- **AND** a new API request is made with `depth=50`

#### Scenario: Empty orderbook
- **WHEN** the selected asset has no book data (`has_book: false`)
- **THEN** the orderbook viewer shows a message "No book data available for this asset" instead of an empty chart

### Requirement: Orderbook WebSocket streaming
The orderbook viewer SHALL use the WebSocket stream (`/api/v1/streams/orderbook`) when in `api` mode for real-time updates. WebSocket updates SHALL be throttled via `requestAnimationFrame` to prevent rendering more than once per frame. The viewer SHALL fall back to HTTP polling if WebSocket is unavailable.

#### Scenario: Real-time update via WebSocket
- **WHEN** the WebSocket receives a new orderbook snapshot message
- **THEN** the price ladder and depth chart update with the new bid/ask levels within one animation frame

#### Scenario: WebSocket reconnection
- **WHEN** the WebSocket connection drops
- **THEN** the viewer continues displaying the last known snapshot and attempts reconnection with exponential backoff (500ms base, 10s max, jitter)
- **AND** a "Reconnecting..." indicator appears in the transport status

### Requirement: Asset selection with summary metrics
The orderbook viewer and ops dashboard SHALL display summary metrics for the selected asset: best bid, best ask, mid price, spread, bid depth, ask depth, sequence number, and last update timestamp. These metrics SHALL be displayed in a compact metric grid above the visualization.

#### Scenario: Metric display
- **WHEN** an asset's orderbook snapshot is loaded
- **THEN** all 8 summary metrics (best bid, best ask, mid, spread, bid depth, ask depth, sequence, last update) are displayed in a metric grid
- **AND** prices are formatted to 4 decimal places and sizes to 6 decimal places

#### Scenario: Null mid price
- **WHEN** the orderbook has bids but no asks (or vice versa)
- **THEN** mid price and spread display as "---" rather than showing a computed value
