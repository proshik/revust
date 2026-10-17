//! Prior-round metadata for prompt context. Source prose is never read into a summary.
use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
#[derive(Default, Deserialize)]
#[serde(default)]
struct Observation {
    findings: Option<Vec<Option<Severity>>>,
    sources: Option<SourceCounts>,
    stats: Option<Timestamps>,
}
#[derive(Deserialize)]
struct Severity {
    severity: Option<String>,
}
#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct SourceCounts {
    expected: Option<i64>,
    reported: Option<i64>,
    degraded: Option<Vec<Option<String>>>,
}
#[derive(Deserialize)]
struct Timestamps {
    started_at: Option<String>,
}
#[derive(Serialize)]
struct Counts {
    total: usize,
    critical: usize,
    major: usize,
    minor: usize,
}
#[derive(Serialize)]
struct Summary {
    round: String,
    recorded_at: Option<String>,
    report_status: &'static str,
    finding_counts: Option<Counts>,
    sources: Option<SourceCounts>,
}
#[derive(Serialize)]
struct Inventory<'a> {
    archive_root: &'a Path,
    rounds: Vec<Summary>,
}
pub fn inventory(task: &Path) -> Result<String, String> {
    if !task
        .try_exists()
        .map_err(|error| format!("stat {}: {error}", task.display()))?
    {
        return Ok(String::new());
    }
    let mut entries = Vec::new();
    for round in crate::config::catalog::rounds(task)? {
        let directory = task.join(&round);
        let modified = fs::metadata(&directory)
            .and_then(|metadata| metadata.modified())
            .map_err(|error| format!("stat {}: {error}", directory.display()))?;
        let mut order: DateTime<Utc> = modified.into();
        let mut summary = Summary {
            round,
            recorded_at: None,
            report_status: "unreadable",
            finding_counts: None,
            sources: None,
        };
        let observed = fs::read(directory.join("findings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Option<Observation>>(&bytes).ok())
            .map(Option::unwrap_or_default);
        if let Some(observation) = observed {
            let stamp = observation
                .stats
                .as_ref()
                .and_then(|stats| stats.started_at.as_deref());
            let parsed = stamp.map(parse_date);
            // A supplied invalid timestamp invalidates the record; an omitted one
            // leaves usable counts with directory mtime determining ordering.
            if !matches!(parsed, Some(None)) {
                if let Some(Some(date)) = parsed
                    && (date.timestamp() != -62_135_596_800 || date.nanosecond() != 0)
                {
                    order = date;
                    summary.recorded_at = Some(date.format("%Y-%m-%dT%H:%MZ").to_string());
                }
                let findings = observation.findings.unwrap_or_default();
                let mut counts = Counts {
                    total: findings.len(),
                    critical: 0,
                    major: 0,
                    minor: 0,
                };
                for severity in findings
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item.severity)
                {
                    match severity.as_str() {
                        "critical" => counts.critical += 1,
                        "major" => counts.major += 1,
                        "minor" => counts.minor += 1,
                        _ => (),
                    }
                }
                summary.finding_counts = Some(counts);
                summary.sources = Some(observation.sources.unwrap_or_default());
                summary.report_status = "available";
            }
        }
        entries.push((order, summary));
    }
    if entries.is_empty() {
        return Ok(String::new());
    }
    entries.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.round.cmp(&right.1.round))
    });
    serde_json::to_string_pretty(&Inventory {
        archive_root: task,
        rounds: entries.into_iter().map(|(_, summary)| summary).collect(),
    })
    .map_err(|error| error.to_string())
}

/// Parse the timestamp grammar used by saved reports. UTC arithmetic permits
/// boundary offsets without requiring a platform-local time zone database.
pub(crate) fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    if !text.is_ascii() {
        return None;
    }
    let (date, clock) = text.split_once('T')?;
    if date.len() != 10 {
        return None;
    }
    let (clock, displacement) = if let Some(clock) = clock.strip_suffix('Z') {
        (clock, 0)
    } else {
        let start = clock.rfind(['+', '-'])?;
        let zone = &clock[start..];
        if zone.len() != 6 || zone.as_bytes()[3] != b':' {
            return None;
        }
        let hours = zone[1..3].parse::<u32>().ok()?;
        let minutes = zone[4..6].parse::<u32>().ok()?;
        if !zone[1..3]
            .bytes()
            .chain(zone[4..6].bytes())
            .all(|byte| byte.is_ascii_digit())
            || hours > 24
            || minutes > 60
        {
            return None;
        }
        let seconds = i64::from(hours * 3600 + minutes * 60);
        (
            &clock[..start],
            if zone.starts_with('-') {
                -seconds
            } else {
                seconds
            },
        )
    };
    let (hour, remaining) = clock.split_once(':')?;
    let normalized = format!("{date}T{hour:0>2}:{}Z", remaining.replace(',', "."));
    let instant = DateTime::parse_from_rfc3339(&normalized)
        .ok()?
        .with_timezone(&Utc);
    if instant.nanosecond() >= 1_000_000_000 {
        return None;
    }
    instant.checked_sub_signed(chrono::TimeDelta::seconds(displacement))
}
