# l2-book-engine Spec

## Snapshot Application

**Given** an empty `L2Book`
**When** `apply_snapshot` is called with 3 bid levels and 2 ask levels
**Then** `bid_depth()` returns 3 and `ask_depth()` returns 2
**And** the sequence and timestamp are updated

**Given** a book with existing levels
**When** `apply_snapshot` is called with new data
**Then** all previous levels are cleared and replaced with the new snapshot
**And** `bid_depth()` and `ask_depth()` reflect only the new data

**Given** a snapshot containing a level with size zero
**When** `apply_snapshot` is called
**Then** that level is not inserted into the book

## Delta Application

### Update existing level

**Given** a book with a bid at 0.50 with size 100
**When** `apply_delta` is called for Side::Bid at price 0.50 with size 500
**Then** the bid at 0.50 now has size 500
**And** the sequence and timestamp are updated

### Remove level

**Given** a book with 3 bid levels including one at 0.50
**When** `apply_delta` is called for Side::Bid at price 0.50 with size 0
**Then** `bid_depth()` returns 2
**And** the level at 0.50 no longer exists in the book

### Add new level

**Given** a book with asks at 0.55 and 0.56
**When** `apply_delta` is called for Side::Ask at price 0.52 with size 75
**Then** `ask_depth()` returns 3
**And** the new best ask is at price 0.52

## Best Bid and Ask

**Given** a book with bids at 0.50, 0.49, 0.48
**When** `best_bid()` is called
**Then** it returns `(FixedPrice(5000), _)` -- the highest bid

**Given** a book with asks at 0.55, 0.56
**When** `best_ask()` is called
**Then** it returns `(FixedPrice(5500), _)` -- the lowest ask

**Given** an empty book
**When** `best_bid()` or `best_ask()` is called
**Then** it returns `None`

## Mid Price

**Given** a book with best bid 0.50 and best ask 0.55
**When** `mid_price()` is called
**Then** it returns `Some(0.525)`

**Given** an empty book (no bids or no asks)
**When** `mid_price()` is called
**Then** it returns `None`

## Spread

**Given** a book with best bid 0.50 and best ask 0.55
**When** `spread()` is called
**Then** it returns `Some(0.05)`

**Given** an empty book
**When** `spread()` is called
**Then** it returns `None`

## Sorted Export

**Given** a book with bids at 0.50, 0.49, 0.48
**When** `bids_sorted()` is called
**Then** the result is ordered highest-to-lowest: [0.50, 0.49, 0.48]

**Given** a book with asks at 0.55, 0.56
**When** `asks_sorted()` is called
**Then** the result is ordered lowest-to-highest: [0.55, 0.56]

## Sequence Gap Detection

**Given** a book with sequence 1
**When** `check_sequence(Sequence(2))` is called
**Then** it returns `Ok(())`

**Given** a book with sequence 1
**When** `check_sequence(Sequence(5))` is called
**Then** it returns `Err(BookError::SequenceGap { expected: 2, got: 5 })`

**Given** a book with sequence 0 (initial state)
**When** `check_sequence` is called with any sequence
**Then** it returns `Ok(())` (no gap check on initial state)
