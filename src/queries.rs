//! Flux queries for each route, and the pure functions that turn their rows into responses.
//!
//! Delivers FR-FW-6 (feeds) and NFR-2 (health). Flux takes `last()` of every series; Rust then keeps,
//! per feed and measurement, only the fields written at the newest time. That drops a stale optional
//! field (such as `reference_deviation_pct`, absent from the newest point) and an older `status` series.
//!
//! Every value put into a Flux string is either a [`Chain`], a feed ID checked by [`valid_feed_id`],
//! or the configured bucket, and all of them go through [`flux_string`].

use std::collections::BTreeMap;

use serde::Serialize;

use crate::influx::Row;

/// The chains Feedwatch reads, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    /// Ethereum mainnet.
    Ethereum,
    /// Arbitrum One.
    Arbitrum,
    /// Base.
    Base,
}

impl Chain {
    /// Every chain, in display order.
    pub const ALL: [Chain; 3] = [Chain::Ethereum, Chain::Arbitrum, Chain::Base];

    /// The `chain` tag value.
    pub fn as_str(self) -> &'static str {
        match self {
            Chain::Ethereum => "ethereum",
            Chain::Arbitrum => "arbitrum",
            Chain::Base => "base",
        }
    }

    /// Parses a `chain` tag or path segment.
    pub fn parse(text: &str) -> Option<Chain> {
        Chain::ALL.into_iter().find(|c| c.as_str() == text)
    }
}

/// Whether `id` can be a manifest feed key: 1 to 64 of `A-Z a-z 0-9 _ . -`.
pub fn valid_feed_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// `text` as a Flux string literal, with `\`, `"` and `${` escaped.
pub fn flux_string(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "\\${");
    format!("\"{escaped}\"")
}

/// Latest `feed_round` and `feed_check` per series over the last `window_s` seconds, plus the
/// last `poll_cycle`, optionally for one chain or one feed.
pub fn latest_query(
    bucket: &str,
    chain: Option<Chain>,
    feed_id: Option<&str>,
    window_s: u64,
) -> String {
    let mut filters = String::new();
    if let Some(chain) = chain {
        filters.push_str(&format!(" and r.chain == {}", flux_string(chain.as_str())));
    }
    if let Some(id) = feed_id {
        filters.push_str(&format!(" and r.feed_id == {}", flux_string(id)));
    }
    format!(
        r#"from(bucket: {bucket})
  |> range(start: -{window_s}s)
  |> filter(fn: (r) => ((r._measurement == "feed_round" or r._measurement == "feed_check"){filters}) or (r._measurement == "poll_cycle" and r._field == "duration_ms"))
  |> last()"#,
        bucket = flux_string(bucket),
    )
}

/// The last `poll_cycle` in the past hour, for `/api/health`.
pub fn health_query(bucket: &str) -> String {
    format!(
        r#"from(bucket: {})
  |> range(start: -1h)
  |> filter(fn: (r) => r._measurement == "poll_cycle" and r._field == "duration_ms")
  |> last()"#,
        flux_string(bucket)
    )
}

/// One feed's `value` and `age_s` over 24 hours, the last of each 5-minute window.
pub fn series_query(bucket: &str, chain: Chain, feed_id: &str) -> String {
    format!(
        r#"from(bucket: {})
  |> range(start: -24h)
  |> filter(fn: (r) => r._measurement == "feed_round" and r.chain == {} and r.feed_id == {} and (r._field == "value" or r._field == "age_s"))
  |> aggregateWindow(every: 5m, fn: last, createEmpty: false)"#,
        flux_string(bucket),
        flux_string(chain.as_str()),
        flux_string(feed_id),
    )
}

/// Parses an RFC 3339 UTC time as InfluxDB writes it (`2026-11-17T23:38:58Z`, optionally with a
/// fraction) into Unix seconds.
pub fn parse_time(text: &str) -> Option<u64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let time = time.split_once('.').map_or(time, |(whole, _)| whole);
    let mut d = date.split('-').map(str::parse::<i64>);
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut t = time.split(':').map(str::parse::<i64>);
    let (h, min, s) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    if d.next().is_some() || t.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || h > 23 || min > 59 || s > 60 {
        return None;
    }
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3_600 + min * 60 + s).ok()
}

/// A feed's latest round and check.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedSummary {
    /// The chain the feed is on.
    pub chain: Chain,
    /// The manifest key, such as `ETH`.
    pub feed_id: String,
    /// `ok`, `stale`, `drift` or `error`; `unknown` if no check was found.
    pub status: String,
    /// When the latest check was written, Unix seconds.
    pub checked_at: Option<u64>,
    /// The price, scaled by the feed's decimals. Absent when the feed couldn't be read.
    pub value: Option<f64>,
    /// The raw integer answer.
    pub answer_raw: Option<String>,
    /// The round ID.
    pub round_id: Option<String>,
    /// When the feed was last updated on chain, Unix seconds.
    pub updated_at: Option<u64>,
    /// Seconds since that update, at the time of the read.
    pub age_s: Option<u64>,
    /// The heartbeat (or daily period for cron feeds), in seconds.
    pub heartbeat_s: Option<u64>,
    /// The deviation trigger, in percent; absent for cron feeds.
    pub deviation_threshold_pct: Option<f64>,
    /// How far the feed is from the reference price, in percent.
    pub reference_deviation_pct: Option<f64>,
    /// Where the reference price came from.
    pub reference_source: Option<String>,
}

/// The newest point per `(chain, feed_id, measurement)`: its time and fields.
type Newest<'a> = BTreeMap<(Chain, &'a str, &'a str), (u64, BTreeMap<&'a str, &'a Row>)>;

/// Builds one [`FeedSummary`] per feed from [`latest_query`] rows, sorted by chain then feed ID.
pub fn latest_by_feed(rows: &[Row]) -> Vec<FeedSummary> {
    let mut feeds: BTreeMap<(Chain, &str), FeedSummary> = BTreeMap::new();
    for ((chain, feed, measurement), (time, fields)) in newest_points(rows) {
        let summary = feeds
            .entry((chain, feed))
            .or_insert_with(|| FeedSummary::empty(chain, feed));
        summary.apply(measurement, time, &fields);
    }
    feeds.into_values().collect()
}

/// Groups `feed_round` and `feed_check` rows by feed and measurement, keeping only the fields
/// written at the newest time.
fn newest_points(rows: &[Row]) -> Newest<'_> {
    let mut newest: Newest<'_> = BTreeMap::new();
    for row in rows {
        let (Some(measurement), Some(chain), Some(feed), Some(field), Some(time)) = (
            cell(row, "_measurement"),
            cell(row, "chain").and_then(Chain::parse),
            cell(row, "feed_id"),
            cell(row, "_field"),
            cell(row, "_time").and_then(parse_time),
        ) else {
            continue;
        };
        if measurement != "feed_round" && measurement != "feed_check" {
            continue;
        }
        let entry = newest
            .entry((chain, feed, measurement))
            .or_insert_with(|| (time, BTreeMap::new()));
        if time > entry.0 {
            *entry = (time, BTreeMap::new());
        }
        if time == entry.0 {
            entry.1.insert(field, row);
        }
    }
    newest
}

impl FeedSummary {
    fn empty(chain: Chain, feed_id: &str) -> FeedSummary {
        FeedSummary {
            chain,
            feed_id: feed_id.to_owned(),
            status: "unknown".to_owned(),
            checked_at: None,
            value: None,
            answer_raw: None,
            round_id: None,
            updated_at: None,
            age_s: None,
            heartbeat_s: None,
            deviation_threshold_pct: None,
            reference_deviation_pct: None,
            reference_source: None,
        }
    }

    /// Fills in the fields of one measurement's newest point.
    fn apply(&mut self, measurement: &str, time: u64, fields: &BTreeMap<&str, &Row>) {
        let value = |name: &str| fields.get(name).and_then(|row| cell(row, "_value"));
        if measurement == "feed_round" {
            self.value = value("value").and_then(|v| v.parse().ok());
            self.answer_raw = value("answer_raw").map(str::to_owned);
            self.round_id = value("round_id").map(str::to_owned);
            self.updated_at = value("updated_at").and_then(|v| v.parse().ok());
            self.age_s = value("age_s").and_then(|v| v.parse().ok());
        } else {
            self.status = fields
                .values()
                .find_map(|row| cell(row, "status"))
                .unwrap_or("unknown")
                .to_owned();
            self.checked_at = Some(time);
            self.heartbeat_s = value("heartbeat_s").and_then(|v| v.parse().ok());
            self.deviation_threshold_pct =
                value("deviation_threshold_pct").and_then(|v| v.parse().ok());
            self.reference_deviation_pct =
                value("reference_deviation_pct").and_then(|v| v.parse().ok());
            self.reference_source = value("reference_source").map(str::to_owned);
        }
    }
}

/// The time of the newest `poll_cycle` row, Unix seconds.
pub fn last_poll_at(rows: &[Row]) -> Option<u64> {
    rows.iter()
        .filter(|row| cell(row, "_measurement") == Some("poll_cycle"))
        .filter_map(|row| cell(row, "_time").and_then(parse_time))
        .max()
}

/// One point of a feed's 24-hour series.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesPoint {
    /// End of the 5-minute window, Unix seconds.
    pub t: u64,
    /// The last price in the window.
    pub value: Option<f64>,
    /// The last age in the window, seconds.
    pub age_s: Option<u64>,
}

/// Joins [`series_query`] rows by time, oldest first.
pub fn series_points(rows: &[Row]) -> Vec<SeriesPoint> {
    let mut points: BTreeMap<u64, SeriesPoint> = BTreeMap::new();
    for row in rows {
        let (Some(t), Some(field), Some(value)) = (
            cell(row, "_time").and_then(parse_time),
            cell(row, "_field"),
            cell(row, "_value"),
        ) else {
            continue;
        };
        let point = points.entry(t).or_insert(SeriesPoint {
            t,
            value: None,
            age_s: None,
        });
        match field {
            "value" => point.value = value.parse().ok(),
            "age_s" => point.age_s = value.parse().ok(),
            _ => {}
        }
    }
    points.into_values().collect()
}

fn cell<'a>(row: &'a Row, name: &str) -> Option<&'a str> {
    row.get(name).map(String::as_str).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[(&str, &str)]) -> Row {
        cells
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn round(chain: &str, feed: &str, time: &str, field: &str, value: &str) -> Row {
        row(&[
            ("_measurement", "feed_round"),
            ("chain", chain),
            ("feed_id", feed),
            ("_time", time),
            ("_field", field),
            ("_value", value),
        ])
    }

    fn check(feed: &str, time: &str, status: &str, field: &str, value: &str) -> Row {
        row(&[
            ("_measurement", "feed_check"),
            ("chain", "base"),
            ("feed_id", feed),
            ("status", status),
            ("_time", time),
            ("_field", field),
            ("_value", value),
        ])
    }

    const T1: &str = "2026-11-18T00:00:00Z";
    const T2: &str = "2026-11-18T00:01:00Z";

    #[test]
    fn fr_fw_6_latest_keeps_only_the_newest_point_per_feed() {
        let rows = vec![
            round("base", "ETH", T2, "value", "3120.5"),
            round("base", "ETH", T2, "answer_raw", "312050000000"),
            round("base", "ETH", T2, "round_id", "7"),
            round("base", "ETH", T2, "updated_at", "1794960000"),
            round("base", "ETH", T2, "age_s", "60"),
            // The last reference deviation was written a cycle earlier: it must not leak in.
            check("ETH", T1, "ok", "reference_deviation_pct", "0.4"),
            check("ETH", T2, "ok", "heartbeat_s", "86400"),
            check("ETH", T2, "ok", "deviation_threshold_pct", "0.5"),
            // An older series with another status.
            check("ETH", T1, "drift", "heartbeat_s", "86400"),
        ];
        let feeds = latest_by_feed(&rows);
        assert_eq!(feeds.len(), 1);
        let eth = &feeds[0];
        assert_eq!(eth.status, "ok");
        assert_eq!(eth.value, Some(3120.5));
        assert_eq!(eth.answer_raw.as_deref(), Some("312050000000"));
        assert_eq!(eth.round_id.as_deref(), Some("7"));
        assert_eq!(eth.updated_at, Some(1_794_960_000));
        assert_eq!(eth.age_s, Some(60));
        assert_eq!(eth.heartbeat_s, Some(86_400));
        assert_eq!(eth.deviation_threshold_pct, Some(0.5));
        assert_eq!(eth.reference_deviation_pct, None);
        assert_eq!(eth.checked_at, parse_time(T2));
    }

    #[test]
    fn fr_fw_6_a_feed_that_failed_to_read_has_a_check_but_no_round() {
        let rows = vec![
            check("USUAL", T2, "error", "heartbeat_s", "86400"),
            round("ethereum", "ETH", T2, "value", "3100"),
        ];
        let feeds = latest_by_feed(&rows);
        assert_eq!(feeds.len(), 2);
        assert_eq!(feeds[0].chain, Chain::Ethereum);
        assert_eq!(feeds[0].status, "unknown");
        assert_eq!(feeds[1].feed_id, "USUAL");
        assert_eq!(feeds[1].status, "error");
        assert_eq!(feeds[1].value, None);
    }

    #[test]
    fn fr_fw_6_skips_rows_it_cannot_use() {
        let rows = vec![
            round("solana", "ETH", T2, "value", "1"),
            round("base", "ETH", "yesterday", "value", "1"),
            row(&[
                ("_measurement", "rpc_call"),
                ("chain", "base"),
                ("feed_id", "x"),
                ("_field", "ok"),
                ("_time", T2),
            ]),
            row(&[
                ("_measurement", "poll_cycle"),
                ("_field", "duration_ms"),
                ("_time", T1),
                ("_value", "300"),
            ]),
            row(&[
                ("_measurement", "poll_cycle"),
                ("_field", "duration_ms"),
                ("_time", T2),
                ("_value", "280"),
            ]),
        ];
        assert!(latest_by_feed(&rows).is_empty());
        assert_eq!(last_poll_at(&rows), parse_time(T2));
        assert_eq!(last_poll_at(&[]), None);
    }

    #[test]
    fn fr_fw_6_series_joins_value_and_age_by_time() {
        let rows = vec![
            round("base", "ETH", T2, "age_s", "30"),
            round("base", "ETH", T1, "value", "3000"),
            round("base", "ETH", T2, "value", "3010"),
            round("base", "ETH", T2, "other", "1"),
            round("base", "ETH", "bad", "value", "1"),
        ];
        let points = series_points(&rows);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].value, Some(3000.0));
        assert_eq!(points[0].age_s, None);
        assert_eq!(
            points[1],
            SeriesPoint {
                t: parse_time(T2).unwrap(),
                value: Some(3010.0),
                age_s: Some(30)
            }
        );
    }

    #[test]
    fn parses_influx_times() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-11-18T00:00:00Z"), Some(1_794_960_000));
        assert_eq!(
            parse_time("2024-02-29T12:30:15.123456789Z"),
            Some(1_709_209_815)
        );
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951_868_800));
        for bad in [
            "",
            "2026-11-18T00:00:00",
            "2026-13-01T00:00:00Z",
            "2026-11-18T24:00:00Z",
            "2026-11-18T00:00Z",
            "2026-11-18-01T00:00:00Z",
            "x-11-18T00:00:00Z",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn fr_fw_6_feed_ids_and_flux_strings_are_safe() {
        assert!(valid_feed_id("ETH"));
        assert!(valid_feed_id("pzETH_FUNDAMENTAL"));
        assert!(valid_feed_id("wstETH-ETH.v2"));
        for bad in ["", "ETH\"", "a b", "x) or true", &"A".repeat(65)] {
            assert!(!valid_feed_id(bad), "{bad}");
        }
        assert_eq!(flux_string(r#"a"b\c${d}"#), r#""a\"b\\c\${d}""#);
    }

    #[test]
    fn fr_fw_6_queries_filter_as_asked() {
        let all = latest_query("feedwatch", None, None, 900);
        assert!(all.contains(r#"from(bucket: "feedwatch")"#));
        assert!(all.contains("range(start: -900s)"));
        assert!(!all.contains("r.chain =="));
        let one = latest_query("feedwatch", Some(Chain::Base), Some("ETH"), 900);
        assert!(one.contains(r#"r.chain == "base""#));
        assert!(one.contains(r#"r.feed_id == "ETH""#));
        assert!(health_query("feedwatch").contains("poll_cycle"));
        let series = series_query("feedwatch", Chain::Arbitrum, "ETH");
        assert!(series.contains(r#"r.chain == "arbitrum""#));
        assert!(series.contains("aggregateWindow(every: 5m"));
        assert_eq!(Chain::parse("base"), Some(Chain::Base));
        assert_eq!(Chain::parse("Base"), None);
    }
}
