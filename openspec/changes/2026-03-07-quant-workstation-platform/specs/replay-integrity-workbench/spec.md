# Spec: Replay Integrity Workbench

## Replay Modes

### Scenario: Replay reconstruction is explicitly keyed by timeline mode

```
Given stored market-data events contain both receive and exchange timestamps
When the workstation requests a reconstruction at a timestamp
Then it must specify either `recv_time` or `exchange_time`
And the response identifies which replay mode was used
```

### Scenario: Replay windows preserve continuity metadata

```
Given a requested replay window spans reconnects, sequence gaps, or stale snapshot skips
When the workstation requests replay output
Then continuity events are returned alongside the replayed market-data view
And the consumer can distinguish complete and best-effort windows
```

## Checkpoints And Validation

### Scenario: Checkpoint usage is visible

```
Given a replay reconstruction starts from a stored checkpoint
When the workstation receives the replay response
Then the response indicates that a checkpoint was used
And the consumer can distinguish checkpoint-assisted replay from snapshot-only replay
```

### Scenario: Validation and drift findings are queryable

```
Given replay validation records exist for an asset and time range
When the workstation requests integrity or replay diagnostics
Then the response includes whether replay matched the reference checkpoint or snapshot
And any mismatch summary is queryable for inspection
```

## Integrity Views

### Scenario: Best-effort windows are labeled explicitly

```
Given the replayed data contains one or more continuity boundaries
When the workstation renders the replay or integrity view
Then the response includes an explicit completeness indicator
And the operator is not forced to infer replay confidence from logs or missing rows
```
