# Spec: Query Workbench

## Schema Discovery

### Scenario: Split dataset schemas are visible

```
Given the workstation exposes a query workbench
When a user requests dataset metadata
Then the response identifies the available logical datasets
And the user can inspect the schema of market, integrity, checkpoint, validation, and execution datasets separately
```

## Read-Only Query Execution

### Scenario: Approved read-only SQL can be executed

```
Given query execution is enabled for an approved environment
When the workstation submits a read-only SQL request
Then the query is executed against the configured backend
And the response returns rows, schema, and execution metadata without allowing data mutation
```

### Scenario: Query guards are enforced

```
Given a query exceeds configured row limits, timeout limits, or read-only policy
When the workstation submits the request
Then the server rejects or truncates the request according to policy
And the user receives a clear explanation of the guard that was triggered
```

## Backend Selection

### Scenario: Local and deployed query backends are separated

```
Given the workstation may run locally or in a deployed environment
When query execution is configured
Then local or development workflows may use DuckDB over Parquet
And deployed interactive workflows use an approved serving backend such as ClickHouse
```
