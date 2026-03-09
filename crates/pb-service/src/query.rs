//! Query workbench service: guarded read-only SQL execution.

use std::time::Instant;

use crate::ServiceError;

/// Guard rails for query execution.
#[derive(Debug, Clone)]
pub struct QueryGuard {
    pub max_rows: usize,
    pub timeout_secs: u64,
}

impl Default for QueryGuard {
    fn default() -> Self {
        Self {
            max_rows: 10_000,
            timeout_secs: 30,
        }
    }
}

/// Column metadata for query results.
#[derive(Debug, Clone)]
pub struct QueryColumnInfo {
    pub name: String,
    pub data_type: String,
}

/// Result of a query execution.
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<QueryColumnInfo>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_count: usize,
    pub truncated: bool,
    pub execution_time_ms: u64,
}

/// Dataset schema metadata.
#[derive(Debug, Clone)]
pub struct DatasetSchema {
    pub name: String,
    pub description: String,
    pub columns: Vec<QueryColumnInfo>,
}

/// Write keywords rejected by the query guard.
const WRITE_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE",
    "RENAME", "GRANT", "REVOKE", "ATTACH", "DETACH",
];

/// Validate that SQL is read-only.
fn validate_read_only(sql: &str) -> Result<(), ServiceError> {
    let upper = sql.to_uppercase();
    // Split on whitespace and check each token - handles leading whitespace, comments etc.
    for keyword in WRITE_KEYWORDS {
        // Check if the keyword appears as a standalone word (not within identifiers)
        // Simple approach: check if it appears at the start or after whitespace
        if upper.split_whitespace().any(|word| word == *keyword) {
            return Err(ServiceError::InvalidParams(format!(
                "write operations are not allowed: found {keyword}"
            )));
        }
    }
    Ok(())
}

/// Inject LIMIT clause if not present and max_rows is set.
fn inject_limit(sql: &str, max_rows: usize) -> String {
    let upper = sql.to_uppercase();
    if upper.contains("LIMIT") {
        sql.to_string()
    } else {
        format!("{sql} LIMIT {max_rows}")
    }
}

/// Transport-neutral query execution service.
pub trait QueryService: Send + Sync {
    /// Execute a read-only SQL query with guard rails.
    fn execute_sql(
        &self,
        sql: &str,
        guard: &QueryGuard,
    ) -> impl std::future::Future<Output = Result<QueryResult, ServiceError>> + Send;

    /// List available datasets and their schemas.
    fn list_datasets(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<DatasetSchema>, ServiceError>> + Send;
}

// ---------------------------------------------------------------------------
// ClickHouseQueryService
// ---------------------------------------------------------------------------

/// Query service backed by ClickHouse HTTP API for dynamic SQL execution.
#[derive(Clone)]
pub struct ClickHouseQueryService {
    /// Base ClickHouse HTTP URL with database and format query params pre-baked.
    query_url: String,
    database: String,
    client: reqwest::Client,
}

impl ClickHouseQueryService {
    pub fn new(url: impl Into<String>, database: impl Into<String>) -> Self {
        let url = url.into();
        let database = database.into();
        let query_url = format!(
            "{}/?database={}&default_format=JSONCompact",
            url.trim_end_matches('/'),
            database,
        );
        Self {
            query_url,
            database,
            client: reqwest::Client::new(),
        }
    }
}

/// ClickHouse JSONCompact response format.
#[derive(Debug, serde::Deserialize)]
struct ClickHouseJsonCompact {
    meta: Vec<ClickHouseColumnMeta>,
    data: Vec<Vec<serde_json::Value>>,
    #[allow(dead_code)]
    rows: u64,
}

#[derive(Debug, serde::Deserialize)]
struct ClickHouseColumnMeta {
    name: String,
    #[serde(rename = "type")]
    data_type: String,
}

impl QueryService for ClickHouseQueryService {
    async fn execute_sql(
        &self,
        sql: &str,
        guard: &QueryGuard,
    ) -> Result<QueryResult, ServiceError> {
        validate_read_only(sql)?;
        let guarded_sql = inject_limit(sql, guard.max_rows);

        let start = Instant::now();
        let resp: reqwest::Response = tokio::time::timeout(
            std::time::Duration::from_secs(guard.timeout_secs),
            self.client
                .post(&self.query_url)
                .body(guarded_sql)
                .send(),
        )
        .await
        .map_err(|_| ServiceError::Internal("query timed out".to_string()))?
        .map_err(|e| ServiceError::Internal(format!("ClickHouse request failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Internal(format!(
                "ClickHouse query error: {body}"
            )));
        }

        let result: ClickHouseJsonCompact = resp
            .json()
            .await
            .map_err(|e| {
                ServiceError::Internal(format!("failed to parse ClickHouse response: {e}"))
            })?;

        let execution_time_ms = start.elapsed().as_millis() as u64;
        let row_count = result.data.len();
        let truncated = row_count >= guard.max_rows;

        Ok(QueryResult {
            columns: result
                .meta
                .into_iter()
                .map(|m| QueryColumnInfo {
                    name: m.name,
                    data_type: m.data_type,
                })
                .collect(),
            rows: result.data,
            row_count,
            truncated,
            execution_time_ms,
        })
    }

    async fn list_datasets(&self) -> Result<Vec<DatasetSchema>, ServiceError> {
        let sql = format!(
            "SELECT table_name, column_name, data_type \
             FROM information_schema.columns \
             WHERE table_schema = '{}' \
             ORDER BY table_name, ordinal_position",
            self.database
        );

        let resp: reqwest::Response = self
            .client
            .post(&self.query_url)
            .body(sql)
            .send()
            .await
            .map_err(|e| ServiceError::Internal(format!("ClickHouse request failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Internal(format!(
                "ClickHouse schema query error: {body}"
            )));
        }

        let result: ClickHouseJsonCompact = resp
            .json()
            .await
            .map_err(|e| {
                ServiceError::Internal(format!("failed to parse schema response: {e}"))
            })?;

        // Group columns by table name.
        let mut datasets: Vec<DatasetSchema> = Vec::new();
        for row in &result.data {
            let table_name = row
                .first()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let column_name = row
                .get(1)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let data_type = row
                .get(2)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let col = QueryColumnInfo {
                name: column_name,
                data_type,
            };

            if let Some(ds) = datasets.last_mut().filter(|d| d.name == table_name) {
                ds.columns.push(col);
            } else {
                datasets.push(DatasetSchema {
                    name: table_name.clone(),
                    description: table_name,
                    columns: vec![col],
                });
            }
        }

        Ok(datasets)
    }
}

// ---------------------------------------------------------------------------
// AnyQueryService dispatch enum
// ---------------------------------------------------------------------------

/// Dispatch enum for query service backends.
#[derive(Clone)]
pub enum AnyQueryService {
    ClickHouse(ClickHouseQueryService),
}

impl QueryService for AnyQueryService {
    async fn execute_sql(
        &self,
        sql: &str,
        guard: &QueryGuard,
    ) -> Result<QueryResult, ServiceError> {
        match self {
            Self::ClickHouse(s) => s.execute_sql(sql, guard).await,
        }
    }

    async fn list_datasets(&self) -> Result<Vec<DatasetSchema>, ServiceError> {
        match self {
            Self::ClickHouse(s) => s.list_datasets().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_read_only_rejects_write_sql() {
        assert!(validate_read_only("DROP TABLE foo").is_err());
        assert!(validate_read_only("  INSERT INTO foo VALUES (1)").is_err());
        assert!(validate_read_only("DELETE FROM foo WHERE 1=1").is_err());
        assert!(validate_read_only("UPDATE foo SET bar = 1").is_err());
        assert!(validate_read_only("TRUNCATE TABLE foo").is_err());
        assert!(validate_read_only("ALTER TABLE foo ADD COLUMN bar Int32").is_err());
        assert!(validate_read_only("CREATE TABLE foo (id Int32)").is_err());
    }

    #[test]
    fn validate_read_only_allows_select() {
        assert!(validate_read_only("SELECT * FROM foo").is_ok());
        assert!(
            validate_read_only("SELECT count() FROM book_events WHERE asset_id = 'abc'").is_ok()
        );
        assert!(validate_read_only("WITH cte AS (SELECT 1) SELECT * FROM cte").is_ok());
    }

    #[test]
    fn inject_limit_adds_limit_when_missing() {
        let sql = "SELECT * FROM foo";
        let result = inject_limit(sql, 100);
        assert_eq!(result, "SELECT * FROM foo LIMIT 100");
    }

    #[test]
    fn inject_limit_preserves_existing_limit() {
        let sql = "SELECT * FROM foo LIMIT 50";
        let result = inject_limit(sql, 100);
        assert_eq!(result, "SELECT * FROM foo LIMIT 50");
    }

    #[test]
    fn query_guard_defaults() {
        let guard = QueryGuard::default();
        assert_eq!(guard.max_rows, 10_000);
        assert_eq!(guard.timeout_secs, 30);
    }
}
