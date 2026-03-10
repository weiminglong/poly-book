## ADDED Requirements

### Requirement: Replay workbench page
The Replay Lab SHALL be enhanced into a replay workbench with: an asset selector populated from active assets, a timestamp input with human-readable datetime picker (in addition to raw microsecond input), a mode selector (recv_time / exchange_time), configurable depth, and a "Run" button. Results SHALL display the reconstructed orderbook using the same price ladder and depth chart components from the orderbook viewer.

#### Scenario: Replay with datetime picker
- **WHEN** the user selects a datetime from the picker
- **THEN** the corresponding microsecond timestamp is computed and used for the replay query
- **AND** the raw microsecond value is displayed below the picker for verification

#### Scenario: Replay result with checkpoint
- **WHEN** the replay reconstruction used a stored checkpoint
- **THEN** a "Used checkpoint" badge appears in the result summary and the checkpoint boundary is marked in any continuity events

#### Scenario: Side-by-side comparison
- **WHEN** the user clicks "Compare modes"
- **THEN** the workbench fires two reconstruction requests (one recv_time, one exchange_time) and displays results side by side with highlighted differences in price levels

#### Scenario: Replay error handling
- **WHEN** the replay request fails (e.g., no data for the requested timestamp)
- **THEN** an error banner displays the backend error message and the previous result (if any) remains visible

### Requirement: Continuity event timeline
The replay workbench and integrity page SHALL display continuity events on a visual timeline. Each event SHALL be plotted by its `recv_timestamp_us` on a horizontal axis with the event kind shown as a colored marker. Hovering a marker SHALL show the full event details in a tooltip.

#### Scenario: Timeline with multiple events
- **WHEN** a replay result contains 3 continuity events
- **THEN** the timeline shows 3 markers positioned proportionally along the time axis
- **AND** each marker is colored by event kind (reconnect=yellow, gap=red, checkpoint=blue)

#### Scenario: Empty timeline
- **WHEN** a replay result has zero continuity events
- **THEN** the timeline area shows "No continuity events" in muted text

### Requirement: Execution inspector page
The Execution Timeline page SHALL be enhanced into an execution inspector with: an order ID search field, an asset filter, a time window selector, and a results table with expandable rows. Each row SHALL show the event timestamp, order ID, event kind, side, price, size, and status. Expanding a row SHALL reveal the full latency trace as a waterfall visualization.

#### Scenario: Latency waterfall visualization
- **WHEN** the user expands an execution event row
- **THEN** a horizontal waterfall chart appears showing the latency between each stage: market_data_recv → normalization_done → strategy_decision → order_submit → exchange_ack → exchange_fill
- **AND** each segment is labeled with its duration in microseconds
- **AND** null stages are shown as dashed/empty segments

#### Scenario: Filter by order ID
- **WHEN** the user enters an order ID in the search field
- **THEN** only events matching that order ID are displayed
- **AND** the total count reflects the filtered result

#### Scenario: Paginated results
- **WHEN** the execution query returns more than 50 events
- **THEN** the results table shows 50 events per page with pagination controls (previous/next)

### Requirement: Integrity dashboard page
The Integrity page SHALL display a comprehensive data quality dashboard for a given asset and time window. It SHALL show: a completeness indicator (complete vs best_effort), a metrics grid (book events, ingest events, reconnects, gaps, stale skips, validation counts matched/mismatched), a continuity event timeline (shared component with replay), and the query time window.

#### Scenario: Complete dataset
- **WHEN** the integrity summary shows `completeness: "complete"`
- **THEN** a green "Complete" badge is displayed prominently at the top of the dashboard

#### Scenario: Best-effort dataset with gaps
- **WHEN** the integrity summary shows `completeness: "best_effort"` with `gap_count > 0`
- **THEN** a yellow "Best Effort" badge is displayed, the gap count metric is highlighted in warning color, and the continuity events timeline shows gap markers

#### Scenario: Validation mismatch
- **WHEN** `validations_mismatched > 0`
- **THEN** the "Mismatched" metric card is highlighted in error color and a tooltip explains what a validation mismatch means
