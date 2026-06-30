//! Query workbench service: guarded read-only SQL execution.

use std::collections::HashSet;
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
    "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE", "RENAME", "GRANT",
    "REVOKE", "ATTACH", "DETACH",
];

const ALLOWED_ROOT_KEYWORDS: &[&str] = &["SELECT", "WITH", "SHOW", "DESCRIBE", "EXPLAIN"];

/// Workbench-visible datasets. Keep this list in sync with the public API docs
/// and the ClickHouse tables created by `pb-store`.
pub const APPROVED_DATASETS: &[&str] = &[
    "book_events",
    "trade_events",
    "ingest_events",
    "book_checkpoints",
    "replay_validations",
    "execution_events",
];

/// Identifiers that must never appear in a workbench query. These are ClickHouse
/// table functions that perform I/O — an unauthenticated SSRF / arbitrary
/// file-read primitive (`file`, `url`, `s3`, `remote`, ...) — plus the `system`
/// database (credential/metadata disclosure) and the file-exfiltration clauses
/// (`INTO OUTFILE`, `SETTINGS`). A SELECT-rooted query with no write keyword
/// otherwise reaches all of these. This blocklist
/// is defense-in-depth alongside the server-side `readonly=2` enforcement.
const FORBIDDEN_IDENTIFIERS: &[&str] = &[
    // I/O table functions.
    "FILE",
    "URL",
    "URLCLUSTER",
    "S3",
    "S3CLUSTER",
    "REMOTE",
    "REMOTESECURE",
    "HDFS",
    "HDFSCLUSTER",
    "MYSQL",
    "POSTGRESQL",
    "JDBC",
    "ODBC",
    "MONGODB",
    "REDIS",
    "SQLITE",
    "EXECUTABLE",
    "AZUREBLOBSTORAGE",
    "DELTALAKE",
    "ICEBERG",
    "GCS",
    "INPUT",
    // Cross-table / cluster / dictionary readers.
    "REMOTE_SECURE",
    "CLUSTER",
    "CLUSTERALLREPLICAS",
    "MERGE",
    "DICTIONARY",
    // Info-disclosure database and exfiltration clauses.
    "SYSTEM",
    "OUTFILE",
    "INFILE",
    "SETTINGS",
];

/// Sanitize result: the normalized SQL text and whether the input ended in a
/// balanced state (i.e. no unclosed quotes, strings, or comments).
struct SanitizeResult {
    sql: String,
    balanced: bool,
    /// True when the input ends inside a line comment (--...).
    ends_in_line_comment: bool,
}

/// Escape a string for embedding in a single-quoted ClickHouse string literal.
/// ClickHouse uses C-style escaping inside `'...'`, so backslash must be escaped
/// first, then the single quote, to neutralize injection via config-sourced
/// values (e.g. the database name in `list_datasets`).
fn escape_ch_string_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Push a byte-length-preserving blank for a masked char so the sanitized
/// string stays byte-aligned with the original. A newline is kept verbatim (it
/// terminates line comments and is itself a single byte); any other char is
/// replaced by `len_utf8()` spaces. Without this, a multi-byte char inside a
/// quote or comment would collapse to one byte, shifting every later index and
/// making callers that slice the *original* by a sanitized-derived offset (e.g.
/// `inject_limit`) panic on a non-char-boundary.
fn push_blanked(out: &mut String, ch: char) {
    if ch == '\n' {
        out.push('\n');
    } else {
        for _ in 0..ch.len_utf8() {
            out.push(' ');
        }
    }
}

fn push_identifier_char(out: &mut String, ch: char) {
    if ch.is_ascii_alphanumeric() || ch == '_' {
        out.push(ch);
    } else {
        push_blanked(out, ch);
    }
}

fn sanitize_sql(sql: &str) -> SanitizeResult {
    sanitize_sql_with_options(sql, true)
}

fn sanitize_sql_for_keywords(sql: &str) -> SanitizeResult {
    sanitize_sql_with_options(sql, false)
}

fn sanitize_sql_with_options(sql: &str, preserve_quoted_identifiers: bool) -> SanitizeResult {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        Backtick,
        LineComment,
        BlockComment,
    }

    let mut sanitized = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut state = State::Normal;

    while let Some(ch) = chars.next() {
        match state {
            State::Normal => match ch {
                '\'' => {
                    sanitized.push(' ');
                    state = State::SingleQuote;
                }
                '"' => {
                    sanitized.push(' ');
                    state = State::DoubleQuote;
                }
                '`' => {
                    sanitized.push(' ');
                    state = State::Backtick;
                }
                '-' if chars.peek() == Some(&'-') => {
                    sanitized.push(' ');
                    sanitized.push(' ');
                    chars.next();
                    state = State::LineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    sanitized.push(' ');
                    sanitized.push(' ');
                    chars.next();
                    state = State::BlockComment;
                }
                _ => sanitized.push(ch),
            },
            State::SingleQuote => {
                push_blanked(&mut sanitized, ch);
                if ch == '\'' {
                    if chars.peek() == Some(&'\'') {
                        sanitized.push(' ');
                        chars.next();
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::DoubleQuote => {
                if preserve_quoted_identifiers {
                    if ch == '"' {
                        if chars.peek() == Some(&'"') {
                            sanitized.push(' ');
                            chars.next();
                        } else {
                            sanitized.push(' ');
                            state = State::Normal;
                        }
                    } else {
                        push_identifier_char(&mut sanitized, ch);
                    }
                } else {
                    push_blanked(&mut sanitized, ch);
                    if ch == '"' {
                        if chars.peek() == Some(&'"') {
                            sanitized.push(' ');
                            chars.next();
                        } else {
                            state = State::Normal;
                        }
                    }
                }
            }
            State::Backtick => {
                if preserve_quoted_identifiers {
                    if ch == '`' {
                        if chars.peek() == Some(&'`') {
                            sanitized.push(' ');
                            chars.next();
                        } else {
                            sanitized.push(' ');
                            state = State::Normal;
                        }
                    } else {
                        push_identifier_char(&mut sanitized, ch);
                    }
                } else {
                    push_blanked(&mut sanitized, ch);
                    if ch == '`' {
                        if chars.peek() == Some(&'`') {
                            sanitized.push(' ');
                            chars.next();
                        } else {
                            state = State::Normal;
                        }
                    }
                }
            }
            State::LineComment => {
                push_blanked(&mut sanitized, ch);
                if ch == '\n' {
                    state = State::Normal;
                }
            }
            State::BlockComment => {
                push_blanked(&mut sanitized, ch);
                if ch == '*' && chars.peek() == Some(&'/') {
                    sanitized.push(' ');
                    chars.next();
                    state = State::Normal;
                }
            }
        }
    }

    SanitizeResult {
        sql: sanitized,
        balanced: state == State::Normal || state == State::LineComment,
        ends_in_line_comment: state == State::LineComment,
    }
}

fn keyword_tokens(sql: &str) -> impl Iterator<Item = &str> {
    sql.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
}

#[derive(Debug, Clone)]
struct SqlToken {
    text: String,
    start: usize,
    end: usize,
}

fn sql_tokens(sql: &str) -> Vec<SqlToken> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    for (idx, ch) in sql.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            start.get_or_insert(idx);
        } else if let Some(token_start) = start.take() {
            tokens.push(SqlToken {
                text: sql[token_start..idx].to_string(),
                start: token_start,
                end: idx,
            });
        }
    }
    if let Some(token_start) = start {
        tokens.push(SqlToken {
            text: sql[token_start..].to_string(),
            start: token_start,
            end: sql.len(),
        });
    }
    tokens
}

fn has_multiple_statements(sql: &str) -> bool {
    let mut saw_semicolon = false;
    for ch in sql.chars() {
        if ch == ';' {
            saw_semicolon = true;
            continue;
        }
        if saw_semicolon && !ch.is_whitespace() {
            return true;
        }
    }
    false
}

fn is_approved_dataset(name: &str) -> bool {
    APPROVED_DATASETS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(name))
}

fn skip_ascii_ws(sql: &str, mut pos: usize) -> usize {
    let bytes = sql.as_bytes();
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    pos
}

fn read_ascii_ident(sql: &str, pos: usize) -> Option<(String, usize)> {
    let bytes = sql.as_bytes();
    let mut end = pos;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    (end > pos).then(|| (sql[pos..end].to_string(), end))
}

fn skip_balanced_parens(sql: &str, pos: usize) -> usize {
    let bytes = sql.as_bytes();
    if bytes.get(pos) != Some(&b'(') {
        return pos;
    }
    let mut depth = 0usize;
    let mut cur = pos;
    while cur < bytes.len() {
        match bytes[cur] {
            b'(' => depth = depth.saturating_add(1),
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return cur + 1;
                }
            }
            _ => {}
        }
        cur += 1;
    }
    sql.len()
}

fn collect_cte_names(sql: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut pos = skip_ascii_ws(sql, 0);
    let Some((root, after_root)) = read_ascii_ident(sql, pos) else {
        return names;
    };
    if root != "WITH" {
        return names;
    }
    pos = skip_ascii_ws(sql, after_root);

    loop {
        let Some((name, after_name)) = read_ascii_ident(sql, pos) else {
            break;
        };
        if name == "SELECT" {
            break;
        }
        pos = skip_ascii_ws(sql, after_name);
        if sql.as_bytes().get(pos) == Some(&b'(') {
            pos = skip_ascii_ws(sql, skip_balanced_parens(sql, pos));
        }
        let Some((as_keyword, after_as)) = read_ascii_ident(sql, pos) else {
            break;
        };
        if as_keyword != "AS" {
            break;
        }
        names.insert(name);
        pos = skip_ascii_ws(sql, after_as);
        if sql.as_bytes().get(pos) == Some(&b'(') {
            pos = skip_ascii_ws(sql, skip_balanced_parens(sql, pos));
        }
        if sql.as_bytes().get(pos) == Some(&b',') {
            pos = skip_ascii_ws(sql, pos + 1);
            continue;
        }
        break;
    }

    names
}

fn validate_table_ref(
    sql: &str,
    tokens: &[SqlToken],
    token_idx: usize,
    cte_names: &HashSet<String>,
) -> Result<(), ServiceError> {
    let clause = tokens
        .get(token_idx.saturating_sub(1))
        .map(|token| token.text.as_str())
        .unwrap_or("table reference");
    let Some(first) = tokens.get(token_idx) else {
        return Err(ServiceError::InvalidParams(format!(
            "table reference is required after {clause}"
        )));
    };
    let between = &sql[tokens[token_idx - 1].end..first.start];
    if between.contains('(') || first.text == "SELECT" {
        return Ok(());
    }

    let after_first = skip_ascii_ws(sql, first.end);
    if sql.as_bytes().get(after_first) == Some(&b'.') {
        return Err(ServiceError::InvalidParams(
            "qualified table names are not allowed; use the configured workbench database"
                .to_string(),
        ));
    }

    if is_approved_dataset(&first.text) || cte_names.contains(&first.text) {
        return Ok(());
    }

    Err(ServiceError::InvalidParams(format!(
        "dataset is not allowed: {}",
        first.text
    )))
}

fn validate_dataset_scope(sql: &str) -> Result<(), ServiceError> {
    let tokens = sql_tokens(sql);
    let cte_names = collect_cte_names(sql);

    for (idx, token) in tokens.iter().enumerate() {
        match token.text.as_str() {
            "FROM" | "JOIN" => validate_table_ref(sql, &tokens, idx + 1, &cte_names)?,
            "DESCRIBE" => {
                let mut table_idx = idx + 1;
                if tokens.get(table_idx).map(|t| t.text.as_str()) == Some("TABLE") {
                    table_idx += 1;
                }
                validate_table_ref(sql, &tokens, table_idx, &cte_names)?;
            }
            "SHOW" => {
                if let Some((table_kw_idx, _)) = tokens
                    .iter()
                    .enumerate()
                    .skip(idx + 1)
                    .take(4)
                    .find(|(_, candidate)| candidate.text == "TABLE")
                {
                    validate_table_ref(sql, &tokens, table_kw_idx + 1, &cte_names)?;
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Validate that SQL is read-only.
fn validate_read_only(sql: &str) -> Result<(), ServiceError> {
    let result = sanitize_sql_for_keywords(sql);
    if !result.balanced {
        return Err(ServiceError::InvalidParams(
            "SQL has unclosed quote or comment".into(),
        ));
    }
    let upper = result.sql.to_uppercase();
    let trimmed = upper.trim();
    if trimmed.is_empty() {
        return Err(ServiceError::InvalidParams(
            "SQL must not be empty".to_string(),
        ));
    }
    if has_multiple_statements(trimmed) {
        return Err(ServiceError::InvalidParams(
            "multiple SQL statements are not allowed".to_string(),
        ));
    }

    let tokens: Vec<&str> = keyword_tokens(trimmed).collect();
    let Some(root) = tokens.first().copied() else {
        return Err(ServiceError::InvalidParams(
            "SQL must not be empty".to_string(),
        ));
    };
    if !ALLOWED_ROOT_KEYWORDS.contains(&root) {
        return Err(ServiceError::InvalidParams(format!(
            "statement type is not allowed: {root}"
        )));
    }
    for keyword in WRITE_KEYWORDS {
        if tokens.iter().any(|token| token == keyword) {
            return Err(ServiceError::InvalidParams(format!(
                "write operations are not allowed: found {keyword}"
            )));
        }
    }
    let identifier_upper = sanitize_sql(sql).sql.to_uppercase();
    let identifier_tokens: Vec<&str> = keyword_tokens(&identifier_upper).collect();
    for ident in FORBIDDEN_IDENTIFIERS {
        if identifier_tokens.iter().any(|token| token == ident) {
            return Err(ServiceError::InvalidParams(format!(
                "identifier is not allowed: {ident}"
            )));
        }
    }
    Ok(())
}

/// Validate and normalize SQL under the configured query guard.
pub fn guard_sql(sql: &str, guard: &QueryGuard) -> Result<String, ServiceError> {
    validate_read_only(sql)?;
    let upper = sanitize_sql(sql).sql.to_uppercase();
    validate_dataset_scope(upper.trim())?;
    Ok(inject_limit(sql, guard.max_rows))
}

/// Inject LIMIT clause if not present and max_rows is set.
fn inject_limit(sql: &str, max_rows: usize) -> String {
    // Strip everything from the FIRST unquoted semicolon onward, using the
    // sanitized form to identify semicolons in Normal state. `validate_read_only`
    // has already rejected anything with a non-empty trailing statement, so past
    // the first unquoted `;` there is only whitespace and further `;` — cutting
    // there yields the single real statement. (Using the *last* semicolon would
    // keep an intermediate one, e.g. `SELECT 1;;` -> `SELECT 1;`, and appending
    // LIMIT after it would re-introduce a second statement.) This also drops
    // trailing quoted identifiers (e.g. `SHOW;\`foo\``) before LIMIT injection.
    let trimmed = sql.trim_end();
    let sanitized_full = sanitize_sql(trimmed);
    let base = if let Some(pos) = sanitized_full.sql.find(';') {
        trimmed[..pos].trim_end()
    } else {
        trimmed
    };
    let result = sanitize_sql(base);
    let upper = result.sql.to_uppercase();
    if keyword_tokens(&upper).any(|token| token == "LIMIT") {
        base.to_string()
    } else {
        // If the SQL ends inside a line comment (e.g. "SELECT 1 --comment"),
        // a newline is needed so the LIMIT isn't swallowed by the comment.
        let sep = if result.ends_in_line_comment {
            "\n"
        } else {
            " "
        };
        format!("{base}{sep}LIMIT {max_rows}")
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

fn clickhouse_reqwest_error(context: &str, err: reqwest::Error) -> ServiceError {
    ServiceError::Internal(format!("{context}: {}", err.without_url()))
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
        let guarded_sql = guard_sql(sql, guard)?;

        // Server-side enforcement (defense-in-depth beyond the guard), passed as
        // ClickHouse HTTP settings appended to the query URL (which already
        // carries `?database=...`):
        //  - readonly=2 forbids any write/DDL and cannot be downgraded mid-query;
        //  - max_result_rows + result_overflow_mode=break cap returned rows
        //    regardless of any LIMIT the user did or didn't write;
        //  - max_execution_time bounds server-side runtime.
        let url = format!(
            "{}&readonly=2&max_result_rows={}&result_overflow_mode=break\
             &max_execution_time={}&cancel_http_readonly_queries_on_client_close=1",
            self.query_url, guard.max_rows, guard.timeout_secs
        );

        let start = Instant::now();
        let request = self.client.post(&url).body(guarded_sql);

        // Wrap send AND body download in one timeout, so a slow body stream
        // can't bypass the deadline by trickling after headers arrive.
        let exec = async {
            let resp = request
                .send()
                .await
                .map_err(|e| clickhouse_reqwest_error("ClickHouse request failed", e))?;
            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(ServiceError::Internal(format!(
                    "ClickHouse query error: {body}"
                )));
            }
            resp.json::<ClickHouseJsonCompact>()
                .await
                .map_err(|e| clickhouse_reqwest_error("failed to parse ClickHouse response", e))
        };
        let result: ClickHouseJsonCompact = tokio::time::timeout(
            std::time::Duration::from_secs(guard.timeout_secs.saturating_add(2)),
            exec,
        )
        .await
        .map_err(|_| ServiceError::Internal("query timed out".to_string()))??;

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
        // Escape the database name for a single-quoted ClickHouse string literal.
        // It comes from config (PB__STORAGE__CLICKHOUSE_DATABASE), not request
        // input, but interpolating it raw would let a compromised/malformed config
        // value inject SQL here — defense-in-depth.
        let escaped_db = escape_ch_string_literal(&self.database);
        let sql = format!(
            "SELECT table_name, column_name, data_type \
             FROM information_schema.columns \
             WHERE table_schema = '{escaped_db}' \
             ORDER BY table_name, ordinal_position"
        );

        let resp: reqwest::Response = self
            .client
            .post(&self.query_url)
            .body(sql)
            .send()
            .await
            .map_err(|e| clickhouse_reqwest_error("ClickHouse request failed", e))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Internal(format!(
                "ClickHouse schema query error: {body}"
            )));
        }

        let result: ClickHouseJsonCompact = resp
            .json()
            .await
            .map_err(|e| clickhouse_reqwest_error("failed to parse schema response", e))?;

        // Group columns by table name.
        let mut datasets: Vec<DatasetSchema> = Vec::new();
        for row in &result.data {
            let table_name = row
                .first()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !is_approved_dataset(&table_name) {
                continue;
            }
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
    use std::time::Duration;

    #[test]
    fn escape_ch_string_literal_neutralizes_injection() {
        // A benign db name is unchanged.
        assert_eq!(escape_ch_string_literal("poly_book"), "poly_book");
        // A single quote that would close the literal is escaped.
        assert_eq!(escape_ch_string_literal("a'b"), "a\\'b");
        // A classic injection payload cannot break out of the literal.
        let injected = escape_ch_string_literal("x' OR '1'='1");
        assert_eq!(injected, "x\\' OR \\'1\\'=\\'1");
        assert!(
            !injected.contains("' OR '"),
            "quote must not stay unescaped"
        );
        // Backslash is escaped first so it cannot re-enable a following quote.
        assert_eq!(escape_ch_string_literal("a\\'b"), "a\\\\\\'b");
    }

    #[tokio::test]
    async fn clickhouse_reqwest_errors_strip_credential_urls() {
        let err = tokio::time::timeout(
            Duration::from_secs(5),
            reqwest::Client::new()
                .get("http://poly_book:secret-password@127.0.0.1:1")
                .send(),
        )
        .await
        .expect("connection attempt should finish")
        .expect_err("closed local port should fail");

        let rendered = clickhouse_reqwest_error("ClickHouse request failed", err).to_string();
        assert!(rendered.contains("ClickHouse request failed"));
        assert!(
            !rendered.contains("poly_book") && !rendered.contains("secret-password"),
            "credential-bearing URL must be stripped from error: {rendered}"
        );
        assert!(
            !rendered.contains("for url"),
            "reqwest URL context should be removed: {rendered}"
        );
    }

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
    fn inject_limit_handles_trailing_semicolon() {
        let sql = "SELECT * FROM foo;";
        let result = inject_limit(sql, 100);
        assert_eq!(result, "SELECT * FROM foo LIMIT 100");
    }

    #[test]
    fn query_guard_defaults() {
        let guard = QueryGuard::default();
        assert_eq!(guard.max_rows, 10_000);
        assert_eq!(guard.timeout_secs, 30);
    }

    #[test]
    fn validate_read_only_rejects_all_write_keywords() {
        for kw in WRITE_KEYWORDS {
            let sql = format!("{kw} something");
            assert!(validate_read_only(&sql).is_err(), "should reject {kw}");
        }
    }

    #[test]
    fn validate_read_only_case_insensitive() {
        assert!(validate_read_only("drop TABLE foo").is_err());
        assert!(validate_read_only("insert INTO foo VALUES (1)").is_err());
    }

    #[test]
    fn validate_read_only_allows_keyword_within_identifier() {
        // "UPDATED_AT" contains "UPDATE" but is not a standalone keyword
        assert!(validate_read_only("SELECT UPDATED_AT FROM foo").is_ok());
        assert!(validate_read_only("SELECT CREATED_BY FROM foo").is_ok());
    }

    #[test]
    fn validate_read_only_rejects_keyword_with_leading_whitespace() {
        assert!(validate_read_only("   DELETE FROM foo").is_err());
        assert!(validate_read_only("\n\tDROP TABLE x").is_err());
    }

    #[test]
    fn validate_read_only_allows_show_and_describe() {
        assert!(validate_read_only("SHOW TABLES").is_ok());
        assert!(validate_read_only("DESCRIBE TABLE foo").is_ok());
    }

    #[test]
    fn validate_read_only_ignores_keywords_in_strings_and_comments() {
        assert!(validate_read_only("SELECT 'DROP TABLE foo'").is_ok());
        assert!(validate_read_only("SELECT 1 -- DELETE FROM foo").is_ok());
        assert!(validate_read_only("/* INSERT INTO foo */ SELECT 1").is_ok());
    }

    #[test]
    fn validate_read_only_rejects_multiple_statements() {
        let err = validate_read_only("SELECT 1; DELETE FROM foo").unwrap_err();
        match err {
            ServiceError::InvalidParams(msg) => assert!(msg.contains("multiple SQL statements")),
            _ => panic!("expected InvalidParams"),
        }
    }

    #[test]
    fn validate_read_only_rejects_non_read_root_statement() {
        let err = validate_read_only("SYSTEM FLUSH LOGS").unwrap_err();
        match err {
            ServiceError::InvalidParams(msg) => {
                assert!(msg.contains("statement type is not allowed"))
            }
            _ => panic!("expected InvalidParams"),
        }
    }

    #[test]
    fn inject_limit_case_insensitive_check() {
        let sql = "SELECT * FROM foo limit 25";
        let result = inject_limit(sql, 100);
        assert_eq!(result, "SELECT * FROM foo limit 25");
    }

    #[test]
    fn inject_limit_with_zero_max_rows() {
        let sql = "SELECT * FROM foo";
        let result = inject_limit(sql, 0);
        assert_eq!(result, "SELECT * FROM foo LIMIT 0");
    }

    #[test]
    fn guard_sql_validates_and_injects_limit() {
        let guard = QueryGuard {
            max_rows: 25,
            timeout_secs: 1,
        };
        let result = guard_sql("SELECT * FROM book_events;", &guard).unwrap();
        assert_eq!(result, "SELECT * FROM book_events LIMIT 25");
    }

    #[test]
    fn guard_sql_rejects_write_statement() {
        let guard = QueryGuard::default();
        let err = guard_sql("DELETE FROM foo", &guard).unwrap_err();
        match err {
            ServiceError::InvalidParams(msg) => {
                assert!(msg.contains("statement type is not allowed"))
            }
            _ => panic!("expected InvalidParams"),
        }
    }

    #[test]
    fn guard_rejects_io_table_functions_and_system() {
        // SELECT-rooted SSRF / file-read / info-disclosure attempts must all be
        // rejected by the identifier blocklist.
        let guard = QueryGuard::default();
        let attacks = [
            "SELECT * FROM file('/etc/passwd', 'CSV')",
            "SELECT * FROM url('http://169.254.169.254/latest/meta-data', 'CSV')",
            "SELECT * FROM s3('https://bucket/key', 'CSV')",
            "SELECT * FROM remote('1.2.3.4:9000', default, t)",
            "SELECT * FROM system.users",
            "SELECT name FROM mysql('host:3306', 'db', 't', 'u', 'p')",
            "SELECT 1 INTO OUTFILE '/tmp/x'",
            "WITH x AS (SELECT * FROM file('/etc/passwd')) SELECT * FROM x",
        ];
        for sql in attacks {
            assert!(
                guard_sql(sql, &guard).is_err(),
                "guard must reject dangerous query: {sql}"
            );
        }
    }

    #[test]
    fn guard_rejects_quoted_io_table_functions_and_system() {
        let guard = QueryGuard::default();
        for sql in [
            "SELECT * FROM `file`('/etc/passwd', 'CSV')",
            "SELECT * FROM \"url\"('http://169.254.169.254/latest/meta-data', 'CSV')",
            "SELECT * FROM `system`.`users`",
        ] {
            assert!(
                guard_sql(sql, &guard).is_err(),
                "guard must reject quoted dangerous identifier: {sql}"
            );
        }
    }

    #[test]
    fn guard_rejects_unadvertised_tables() {
        let guard = QueryGuard::default();
        for sql in [
            "SELECT * FROM hidden_table",
            "SHOW CREATE TABLE hidden_table",
            "SELECT * FROM poly_book.book_events",
            "WITH cte AS (SELECT * FROM system.users) SELECT * FROM cte",
        ] {
            assert!(
                guard_sql(sql, &guard).is_err(),
                "guard must reject query outside approved dataset scope: {sql}"
            );
        }
    }

    #[test]
    fn guard_rejects_dangling_table_references() {
        let guard = QueryGuard::default();
        for sql in [
            "SELECT * FROM",
            "SELECT * FROM book_events JOIN",
            "DESCRIBE TABLE",
            "WITH\0JOIN\0",
        ] {
            let err = match guard_sql(sql, &guard) {
                Ok(accepted) => panic!("guard must reject {sql:?}: {accepted:?}"),
                Err(err) => err,
            };
            match err {
                ServiceError::InvalidParams(msg) => {
                    assert!(
                        msg.contains("table reference is required"),
                        "unexpected error for {sql:?}: {msg}"
                    );
                }
                _ => panic!("expected InvalidParams for {sql:?}"),
            }
        }
    }

    #[test]
    fn guard_rejects_root_keyword_inside_quoted_identifier() {
        let guard = QueryGuard::default();
        let err = guard_sql("\u{2}\"SHOW%IW\0\"\"\";;;;;;;;;;;;;;;;;;;;;;;;", &guard).unwrap_err();
        match err {
            ServiceError::InvalidParams(msg) => {
                assert!(
                    msg.contains("statement type is not allowed")
                        || msg.contains("SQL must not be empty"),
                    "unexpected error: {msg}"
                );
            }
            _ => panic!("expected InvalidParams"),
        }
    }

    #[test]
    fn guard_allows_cte_over_approved_dataset() {
        let guard = QueryGuard::default();
        assert!(guard_sql(
            "WITH cte AS (SELECT * FROM book_events) SELECT * FROM cte",
            &guard
        )
        .is_ok());
    }

    #[test]
    fn guard_allows_legitimate_dataset_query() {
        let guard = QueryGuard::default();
        assert!(guard_sql("SELECT asset_id, price FROM book_events", &guard).is_ok());
    }

    #[test]
    fn validate_read_only_empty_string() {
        assert!(validate_read_only("").is_err());
    }

    #[test]
    fn validate_read_only_error_contains_keyword() {
        let err = validate_read_only("DROP TABLE foo").unwrap_err();
        match err {
            ServiceError::InvalidParams(msg) => assert!(msg.contains("DROP")),
            _ => panic!("expected InvalidParams"),
        }
    }

    #[test]
    fn query_guard_custom_values() {
        let guard = QueryGuard {
            max_rows: 500,
            timeout_secs: 5,
        };
        assert_eq!(guard.max_rows, 500);
        assert_eq!(guard.timeout_secs, 5);
    }

    #[test]
    fn guard_sql_rejects_unclosed_quote() {
        let guard = QueryGuard::default();
        let err = guard_sql("SELECT * FROM foo WHERE name = 'unclosed", &guard).unwrap_err();
        match err {
            ServiceError::InvalidParams(msg) => {
                assert!(msg.contains("unclosed"), "got: {msg}")
            }
            _ => panic!("expected InvalidParams"),
        }
    }

    #[test]
    fn guard_sql_rejects_unclosed_double_quote() {
        let guard = QueryGuard::default();
        let err = guard_sql("SELECT * FROM \"foo", &guard).unwrap_err();
        match err {
            ServiceError::InvalidParams(msg) => {
                assert!(msg.contains("unclosed"), "got: {msg}")
            }
            _ => panic!("expected InvalidParams"),
        }
    }

    #[test]
    fn guard_sql_trailing_line_comment_is_idempotent() {
        let guard = QueryGuard {
            max_rows: 100,
            timeout_secs: 1,
        };
        let sql = "SELECT 1 --comment";
        let first = guard_sql(sql, &guard).unwrap();
        let second = guard_sql(&first, &guard).unwrap();
        assert_eq!(first, second, "LIMIT injection must be idempotent");
    }

    /// Property test for the invariant `fuzz_query_guard` enforces: whenever the
    /// first guard pass accepts an input, its output must itself be guard-valid
    /// (a stable, non-empty, single statement). This deterministically sweeps
    /// short strings built from the parser-significant alphabet (semicolons,
    /// quotes, comment markers, newlines, multi-byte chars) crossed with the
    /// boundary `max_rows` values, so a regression is caught in CI without
    /// relying on the stochastic fuzzer. Covers the `SELECT 1;;` (last- vs
    /// first-semicolon) and multi-byte-offset cases that previously crashed.
    #[test]
    fn guard_output_is_always_guard_valid() {
        let prefixes = [
            "SELECT 1",
            "SELECT 1 ",
            "SHOW",
            "SELECT",
            "SELECT '",
            "SELECT \"",
            "SELECT `",
            "DESCRIBE x",
            "SELECT '1'",
            "SELECT /*",
            "SELECT --",
            "SELECT '€'",
            "SELECT 1 LIMIT 5",
            "SELECT 1 LIMIT 5 -",
            "SELECT 1 LIMIT 5 --",
        ];
        let alpha = [
            ' ', ';', '\'', '"', '`', '-', '/', '*', '\n', '(', ')', 'x', '1', '€',
        ];
        for p in prefixes {
            for max_rows in [0usize, 1, 100] {
                let guard = QueryGuard {
                    max_rows,
                    timeout_secs: 1,
                };
                let n = alpha.len();
                for len in 0..=3usize {
                    for idx in 0..n.pow(len as u32) {
                        let mut sql = String::from(p);
                        let mut k = idx;
                        for _ in 0..len {
                            sql.push(alpha[k % n]);
                            k /= n;
                        }
                        // Only inputs the first pass accepts exercise the property.
                        let Ok(first) = guard_sql(&sql, &guard) else {
                            continue;
                        };
                        assert!(
                            !first.trim().is_empty(),
                            "guarded SQL must not be empty: {sql:?} -> {first:?}",
                        );
                        // Idempotence (below) is the meaningful invariant: a
                        // trailing *unquoted* semicolon would be stripped on the
                        // second pass, breaking equality. A `;` inside a trailing
                        // comment/string is harmless and intentionally retained.
                        let second = guard_sql(&first, &guard).unwrap_or_else(|e| {
                            panic!(
                                "re-guarding accepted output failed: {sql:?} -> {first:?}: {e:?}"
                            )
                        });
                        assert_eq!(
                            first, second,
                            "guard must be idempotent: {sql:?} -> {first:?} != {second:?}",
                        );
                    }
                }
            }
        }
    }

    /// Regression: a multi-byte char inside a quote or comment must not shift
    /// the byte offsets `inject_limit` uses to slice the original SQL. Before
    /// `sanitize_sql` became byte-length-preserving, the sanitized string was
    /// shorter than the original (each multi-byte char collapsed to one byte),
    /// so `trimmed[..pos]` either panicked on a non-char-boundary or sliced
    /// mid-quote and produced an unbalanced statement that failed re-guarding.
    #[test]
    fn guard_sql_multibyte_in_quote_is_balanced_and_idempotent() {
        let guard = QueryGuard {
            max_rows: 100,
            timeout_secs: 1,
        };
        // Each of these passes the first guard pass; the result must itself be
        // guard-valid (the property the fuzz target `fuzz_query_guard` checks).
        for sql in [
            "SELECT \"€\" ;",
            "SELECT 'π' ;",
            "SELECT 1 /* € */ ;",
            "SELECT 1 --€\n;",
            "SELECT `名` ;",
        ] {
            let first = guard_sql(sql, &guard)
                .unwrap_or_else(|e| panic!("first guard pass failed for {sql:?}: {e:?}"));
            let second = guard_sql(&first, &guard)
                .unwrap_or_else(|e| panic!("second guard pass failed for {sql:?}: {e:?}"));
            assert_eq!(first, second, "guard must be idempotent for {sql:?}");
            assert!(
                !first.trim_end().ends_with(';'),
                "guarded SQL must not keep a trailing semicolon for {sql:?}",
            );
        }
    }
}
