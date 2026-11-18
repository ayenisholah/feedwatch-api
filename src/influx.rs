//! InfluxDB 2 client: runs Flux queries and parses the annotated CSV they return.
//!
//! Part of FR-FW-6. Parsing is a pure function ([`parse_annotated_csv`]); HTTP sits behind
//! [`InfluxQuery`] so routes can be tested with a fake store.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::json;
use thiserror::Error;

/// InfluxDB 2 connection settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfluxConfig {
    /// Base URL, such as `http://influxdb:8086`, without a trailing slash.
    pub url: String,
    /// Organisation name.
    pub org: String,
    /// Bucket the poller writes to.
    pub bucket: String,
    /// API token with read access to the bucket.
    pub token: String,
}

/// One result row: column name to raw cell text. Annotation columns are dropped.
pub type Row = HashMap<String, String>;

/// Why a query failed.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum InfluxError {
    /// InfluxDB couldn't be reached, or the connection failed mid-response.
    #[error("influxdb unreachable: {0}")]
    Unreachable(String),
    /// InfluxDB answered with a non-success HTTP status.
    #[error("influxdb returned HTTP {status}: {body}")]
    Http {
        /// The HTTP status code.
        status: u16,
        /// The response body, for the log.
        body: String,
    },
    /// The query ran but reported an error inside the result.
    #[error("flux error: {0}")]
    Query(String),
}

/// A boxed, sendable future; keeps [`InfluxQuery`] usable as `dyn`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Runs Flux queries. [`HttpInflux`] is the real one; route tests use a fake.
pub trait InfluxQuery: Send + Sync {
    /// Runs `flux` and returns every row of every table.
    fn query<'a>(&'a self, flux: &'a str) -> BoxFuture<'a, Result<Vec<Row>, InfluxError>>;
}

/// [`InfluxQuery`] over the InfluxDB 2 HTTP API.
#[derive(Debug, Clone)]
pub struct HttpInflux {
    client: reqwest::Client,
    config: InfluxConfig,
}

impl HttpInflux {
    /// A client with a per-request timeout.
    pub fn new(config: InfluxConfig, timeout: Duration) -> Result<HttpInflux, InfluxError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| InfluxError::Unreachable(e.to_string()))?;
        Ok(HttpInflux { client, config })
    }

    /// The configured bucket, for building queries.
    pub fn bucket(&self) -> &str {
        &self.config.bucket
    }

    async fn run(&self, flux: &str) -> Result<Vec<Row>, InfluxError> {
        let response = self
            .client
            .post(format!("{}/api/v2/query", self.config.url))
            .query(&[("org", &self.config.org)])
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Token {}", self.config.token),
            )
            .header(reqwest::header::ACCEPT, "application/csv")
            .json(&json!({
                "query": flux,
                "type": "flux",
                "dialect": {"annotations": ["datatype", "group", "default"]},
            }))
            .send()
            .await
            .map_err(|e| InfluxError::Unreachable(e.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| InfluxError::Unreachable(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(InfluxError::Http { status, body });
        }
        parse_annotated_csv(&body)
    }
}

impl InfluxQuery for HttpInflux {
    fn query<'a>(&'a self, flux: &'a str) -> BoxFuture<'a, Result<Vec<Row>, InfluxError>> {
        Box::pin(self.run(flux))
    }
}

/// Parses InfluxDB annotated CSV into rows.
///
/// Handles several tables separated by blank lines, `#` annotation rows, and RFC 4180 quoting
/// (commas, doubled quotes and line breaks inside quoted cells). A table with a non-empty
/// `error` column is returned as [`InfluxError::Query`].
pub fn parse_annotated_csv(csv: &str) -> Result<Vec<Row>, InfluxError> {
    let mut rows = Vec::new();
    let mut header: Option<Vec<String>> = None;
    for record in records(csv) {
        let blank = record.iter().all(String::is_empty);
        if blank {
            header = None;
            continue;
        }
        if record.first().is_some_and(|c| c.starts_with('#')) {
            continue;
        }
        let Some(columns) = &header else {
            header = Some(record);
            continue;
        };
        let row: Row = columns
            .iter()
            .zip(record)
            .filter(|(name, _)| !name.is_empty())
            .map(|(name, cell)| (name.clone(), cell))
            .collect();
        if let Some(message) = row.get("error").filter(|m| !m.is_empty()) {
            return Err(InfluxError::Query(message.clone()));
        }
        rows.push(row);
    }
    Ok(rows)
}

/// Splits CSV text into records of unquoted cells. `\r\n` and `\n` both end a record.
fn records(csv: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut record = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = csv.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (true, _) => cell.push(c),
            (false, '"') => quoted = true,
            (false, ',') => record.push(std::mem::take(&mut cell)),
            (false, '\r') if chars.peek() == Some(&'\n') => {}
            (false, '\n') => {
                record.push(std::mem::take(&mut cell));
                out.push(std::mem::take(&mut record));
            }
            (false, _) => cell.push(c),
        }
    }
    if !cell.is_empty() || !record.is_empty() {
        record.push(cell);
        out.push(record);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr_fw_6_parses_several_tables_and_drops_annotations() {
        let csv = "#datatype,string,long,string,double\r\n\
                   #group,false,false,true,false\r\n\
                   #default,_result,,,\r\n\
                   ,result,table,chain,_value\r\n\
                   ,,0,base,1.5\r\n\
                   ,,0,base,2.5\r\n\
                   \r\n\
                   #datatype,string,long,string\r\n\
                   ,result,table,feed_id\r\n\
                   ,,1,ETH\r\n";
        let rows = parse_annotated_csv(csv).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["_value"], "2.5");
        assert_eq!(rows[2]["feed_id"], "ETH");
        assert!(!rows[2].contains_key("chain"));
        assert!(!rows[0].contains_key(""));
    }

    #[test]
    fn fr_fw_6_parses_quoted_cells() {
        let csv = ",result,table,detail\n,,0,\"age 90s, heartbeat 60s\"\n,,0,\"said \"\"hi\"\"\nthen left\"\n";
        let rows = parse_annotated_csv(csv).unwrap();
        assert_eq!(rows[0]["detail"], "age 90s, heartbeat 60s");
        assert_eq!(rows[1]["detail"], "said \"hi\"\nthen left");
    }

    #[test]
    fn fr_fw_6_empty_result_has_no_rows() {
        assert_eq!(parse_annotated_csv(""), Ok(vec![]));
        assert_eq!(parse_annotated_csv("\r\n\r\n"), Ok(vec![]));
        let header_only = ",result,table,_value\r\n";
        assert_eq!(parse_annotated_csv(header_only), Ok(vec![]));
    }

    #[test]
    fn fr_fw_6_reports_an_error_table() {
        let csv = "#datatype,string,string\r\n,error,reference\r\n,\"undefined identifier x\",\r\n";
        assert_eq!(
            parse_annotated_csv(csv),
            Err(InfluxError::Query("undefined identifier x".to_owned()))
        );
    }

    #[test]
    fn last_record_without_newline_is_kept() {
        let rows = parse_annotated_csv(",a\n,1").unwrap();
        assert_eq!(rows[0]["a"], "1");
    }
}
