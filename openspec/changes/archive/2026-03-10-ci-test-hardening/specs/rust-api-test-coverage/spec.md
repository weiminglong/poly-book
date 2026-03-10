## ADDED Requirements

### Requirement: pb-api route handler unit tests
The `pb-api` crate SHALL have unit tests for HTTP route handlers covering error
paths and input validation.

#### Scenario: Invalid asset ID returns 404
- **WHEN** a request is made to `/api/v1/orderbooks/{asset_id}/snapshot` with a
  non-existent asset ID
- **THEN** the handler returns HTTP 404 with a JSON error body

#### Scenario: Malformed replay parameters return 400
- **WHEN** a request is made to `/api/v1/replay/reconstruct` with missing or
  invalid query parameters
- **THEN** the handler returns HTTP 400 with a descriptive error message

#### Scenario: Missing query body returns 400
- **WHEN** a POST request is made to `/api/v1/query/sql` with no request body
- **THEN** the handler returns HTTP 400

#### Scenario: Feed status returns valid response with mock service
- **WHEN** the feed status handler is called with a mock BookService
- **THEN** it returns HTTP 200 with a response matching the FeedStatusResponse
  schema
