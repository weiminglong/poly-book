# Spec: Strategy Control Plane

## Strategy Lifecycle

### Scenario: Strategy state transitions are explicit

```
Given a future workstation allows operators to manage strategies
When a strategy is created, deployed, paused, resumed, or stopped
Then each strategy state transition is explicit and queryable
And the workstation does not rely on implicit UI-only state to represent runtime truth
```

### Scenario: Strategy configuration is versioned

```
Given a future strategy is parameterized by code, configuration, or model inputs
When the strategy is deployed or updated
Then the deployment references a versioned strategy definition and parameter set
And operators can identify exactly which version is running in each environment
```

## Audit And Provenance

### Scenario: Tuning changes preserve provenance

```
Given a future workstation supports parameter tuning or evaluation
When parameters or evaluation settings change
Then the resulting configuration, dataset references, and evaluation outputs are recorded with provenance
And the user can reconstruct how the chosen configuration was produced
```

## Environment Boundaries

### Scenario: Strategy control is environment-scoped

```
Given the workstation may operate across development, simulation, and production environments
When an operator inspects or changes strategy state
Then the action is explicitly scoped to an environment
And strategy state from one environment is not silently confused with another
```
