## ADDED Requirements

### Requirement: Schema browser
The Query Workbench page SHALL include a schema browser sidebar that lists all available datasets fetched from `GET /api/v1/query/datasets`. Each dataset SHALL be expandable to show its columns with name and data type. Clicking a column name SHALL insert it into the SQL editor at the cursor position.

#### Scenario: Dataset listing
- **WHEN** the Query Workbench page loads
- **THEN** the schema browser fetches and displays all available datasets with their descriptions
- **AND** each dataset is expandable to reveal its column schema

#### Scenario: Column insertion
- **WHEN** the user clicks a column name in the schema browser
- **THEN** the column name is inserted at the current cursor position in the SQL editor

#### Scenario: Schema fetch failure
- **WHEN** the datasets endpoint returns an error
- **THEN** the schema browser shows an error message with a "Retry" button

### Requirement: SQL editor
The Query Workbench SHALL include a SQL text editor with syntax highlighting for SQL keywords. The editor SHALL support multi-line input and provide a "Run Query" button (and Cmd+Enter keyboard shortcut) to submit the query to `POST /api/v1/query/sql`. The editor SHALL preserve the last query in `sessionStorage`.

#### Scenario: Query submission
- **WHEN** the user types a SQL query and clicks "Run Query"
- **THEN** the query is submitted to the backend via `POST /api/v1/query/sql`
- **AND** a loading indicator appears while the query executes

#### Scenario: Keyboard shortcut execution
- **WHEN** the user presses Cmd+Enter (or Ctrl+Enter on non-Mac) while the editor is focused
- **THEN** the current query is submitted, identical to clicking "Run Query"

#### Scenario: Query persistence
- **WHEN** the user navigates away from the Query Workbench and returns
- **THEN** the last-entered query is restored from `sessionStorage`

#### Scenario: Empty query prevention
- **WHEN** the user clicks "Run Query" with an empty editor
- **THEN** no request is made and the editor border briefly highlights in warning color

### Requirement: Query results table
Query results SHALL be displayed in a paginated, sortable table below the editor. The table SHALL show column headers from the response's `columns` array and render each row from the `rows` array. The table SHALL indicate when results are truncated by the backend. Execution time SHALL be displayed above the results.

#### Scenario: Successful query with results
- **WHEN** the backend returns a successful query result with 25 rows
- **THEN** the results table displays all 25 rows with correct column headers
- **AND** the execution time is shown (e.g., "Query completed in 42ms")

#### Scenario: Truncated results
- **WHEN** the backend returns `truncated: true`
- **THEN** a warning banner appears above the results: "Results truncated. The query returned more rows than the limit."

#### Scenario: Query error
- **WHEN** the backend returns an error for a malformed SQL query
- **THEN** the error message is displayed in an error banner below the editor
- **AND** the previous results (if any) remain visible

#### Scenario: Sort by column
- **WHEN** the user clicks a column header in the results table
- **THEN** the rows are sorted client-side by that column (ascending on first click, descending on second)
