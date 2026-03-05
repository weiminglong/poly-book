# WebSocket Client Spec

## Connection

### Connects to Polymarket WS endpoint

**Given** a `WsClient` configured with one or more asset IDs
**When** `run()` is called
**Then** a WebSocket connection is established to
`wss://ws-subscriptions-clob.polymarket.com/ws/market`

### Subscribes to each asset

**Given** an open WebSocket connection
**When** the connection is first established
**Then** the client sends a JSON subscription message for each configured
asset ID with `{"type": "subscribe", "channel": "market", "assets_id": "<id>"}`

## Heartbeat

### Sends periodic pings

**Given** an active WebSocket connection
**When** 10 seconds elapse without other send activity
**Then** the client sends a WebSocket Ping frame

### Handles pong responses

**Given** an active WebSocket connection
**When** a Pong frame is received
**Then** the client logs at debug level and continues

## Message Forwarding

### Forwards text messages with timestamp

**Given** an active WebSocket connection and a connected output channel
**When** a Text frame is received
**Then** the client wraps it in a `WsRawMessage` with the current
`recv_timestamp_us` (microseconds since UNIX epoch) and sends it on the
`mpsc` channel

### Stops on receiver drop

**Given** an active WebSocket connection
**When** the output channel receiver has been dropped (send returns error)
**Then** the client logs the error and returns `Ok(())`

## Reconnection

### Reconnects on error with backoff

**Given** a WebSocket connection that fails with an error
**When** the connection loop exits with `Err`
**Then** the client sleeps for `min(100ms * 2^attempt + jitter, 30s)` and
reconnects

### Resets attempt counter on graceful close

**Given** a WebSocket connection that closes gracefully (Close frame or
stream end)
**When** the connection loop exits with `Ok`
**Then** the attempt counter resets to 0 before the next reconnection

### Caps backoff at 30 seconds

**Given** the attempt counter exceeds 15
**When** backoff is computed
**Then** the delay is capped at 30,000 ms

### Adds jitter to backoff

**Given** any reconnection attempt
**When** backoff is computed
**Then** a jitter of up to `exp/4` milliseconds is added, derived from
system clock subsecond nanos

## Close Handling

### Handles close frame

**Given** an active WebSocket connection
**When** a Close frame is received
**Then** the client logs and returns `Ok(())`

### Handles stream end

**Given** an active WebSocket connection
**When** the stream yields `None`
**Then** the client returns `Ok(())`
