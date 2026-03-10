## ADDED Requirements

### Requirement: CodeQL analysis workflow
The repository SHALL have a CodeQL workflow at `.github/workflows/codeql.yml` that
runs static analysis on Rust and TypeScript code for every pull request targeting
main and on every push to main.

#### Scenario: CodeQL runs on pull request
- **WHEN** a pull request is opened against main
- **THEN** CodeQL analysis runs for both Rust and TypeScript languages
- **THEN** results appear as a check on the PR

#### Scenario: CodeQL does not block main CI
- **WHEN** the CodeQL workflow runs
- **THEN** it executes as a separate workflow independent of ci.yml
- **THEN** main CI jobs are not delayed by CodeQL analysis time

### Requirement: Secret scanning enabled
The repository SHALL have GitHub secret scanning enabled to detect accidentally
committed credentials, API keys, and tokens.

#### Scenario: Secret detected in push
- **WHEN** a commit containing a known secret pattern is pushed
- **THEN** GitHub flags the secret and notifies the repository administrator
