## ADDED Requirements

### Requirement: Vitest coverage configuration
The Vitest configuration SHALL include v8 coverage with text and lcov reporters.

#### Scenario: Coverage report generated on test run
- **WHEN** `npm run test:coverage` is executed
- **THEN** a text summary is printed to the terminal
- **THEN** an LCOV report is written to `coverage/lcov.info`

### Requirement: Coverage script in package.json
The `package.json` SHALL include a `test:coverage` script that runs Vitest with
coverage enabled.

#### Scenario: Coverage script available
- **WHEN** `npm run test:coverage` is executed
- **THEN** Vitest runs all tests and generates coverage output

### Requirement: Coverage excludes non-source files
The coverage configuration SHALL exclude test files, type declarations, and
configuration files from coverage measurement.

#### Scenario: Test files excluded from coverage
- **WHEN** coverage is calculated
- **THEN** files in `__tests__/` directories and `*.test.ts(x)` files are not
  counted toward coverage percentages
