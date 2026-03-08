# Spec: Backtest-Credible Replay

## Replay Modes

### Scenario: Replay uses receive-time ordering when requested

```
Given stored events contain both receive and exchange timestamps
When replay is requested with mode `recv_time`
Then events are ordered by local receive-time semantics
And the reconstructed outputs reflect what the local process observed
```

### Scenario: Replay uses exchange-time ordering when requested

```
Given stored events contain both receive and exchange timestamps
When replay is requested with mode `exchange_time`
Then events are ordered by exchange-time semantics
And the consumer is not forced onto receive-time ordering implicitly
```

## Trade Fidelity

### Scenario: Trade events store real size and side when available

```
Given the venue provides a trade update with price, size, and side
When the trade is normalized and persisted
Then the stored `trade_event` includes price, size, side, and stable source identity
And the event is usable for trade-aware backtesting
```

### Scenario: Missing trade fidelity is explicit

```
Given the venue provides only last-trade-price updates without trade size
When the update is persisted
Then the stored event is marked as partial trade information
And downstream consumers can distinguish it from a fill-quality trade record
```

## Checkpoints and Validation

### Scenario: Periodic checkpoints are available for replay

```
Given book events are continuously ingested for an asset
When the checkpoint interval elapses
Then a full L2 snapshot is persisted to `book_checkpoints`
And future replay can resume from the latest checkpoint before the target time
```

### Scenario: Reconstructed books are validated

```
Given a replayed book can be compared against a later stored snapshot or checkpoint
When validation runs
Then drift results are persisted with the asset, timestamps, and mismatch summary
And backtest consumers can reject or flag corrupted replay windows
```
