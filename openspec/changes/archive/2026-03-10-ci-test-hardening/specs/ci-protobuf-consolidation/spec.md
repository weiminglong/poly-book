## ADDED Requirements

### Requirement: Reusable protobuf setup action
The repository SHALL provide a composite action at
`.github/actions/setup-protobuf/action.yml` that installs the protobuf compiler.

#### Scenario: Composite action installs protobuf-compiler
- **WHEN** a CI job uses the `setup-protobuf` composite action
- **THEN** `protoc` is available on the PATH for subsequent build steps

### Requirement: CI jobs use composite action
The `check`, `test`, and `clippy` jobs in `ci.yml` SHALL use the composite action
instead of inline `sudo apt-get install -y protobuf-compiler` commands.

#### Scenario: No inline protobuf installation in ci.yml
- **WHEN** ci.yml is inspected
- **THEN** no job contains a `sudo apt-get install -y protobuf-compiler` step
- **THEN** all jobs that need protobuf use `uses: ./.github/actions/setup-protobuf`
