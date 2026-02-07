// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::fs;
use std::io::Write;

use super::*;

/// Helper: write JSONL lines to a temp file, return path.
fn write_session_log(dir: &Path, lines: &[&str]) -> PathBuf {
    let path = dir.join("session.jsonl");
    let mut f = fs::File::create(&path).unwrap();
    for line in lines {
        writeln!(f, "{}", line).unwrap();
    }
    path
}

#[test]
fn parse_session_usage_extracts_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session_log(
        dir.path(),
        &[
            r#"{"type":"user","message":{"role":"user"},"timestamp":"2026-01-01T00:00:00Z"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet-4-5-20250929","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:01Z"}"#,
            r#"{"type":"result","content":"ok"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet-4-5-20250929","usage":{"input_tokens":200,"output_tokens":75,"cache_creation_input_tokens":0,"cache_read_input_tokens":30},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:02Z"}"#,
        ],
    );

    let (deltas, new_offset) = parse_session_usage(&path, 0);

    assert_eq!(deltas.input_tokens, 300);
    assert_eq!(deltas.output_tokens, 125);
    assert_eq!(deltas.cache_creation_input_tokens, 10);
    assert_eq!(deltas.cache_read_input_tokens, 50);
    assert_eq!(deltas.model.as_deref(), Some("claude-sonnet-4-5-20250929"));
    assert!(new_offset > 0);
}

#[test]
fn parse_session_usage_incremental() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session_log(
        dir.path(),
        &[
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet-4-5-20250929","usage":{"input_tokens":100,"output_tokens":50},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:01Z"}"#,
        ],
    );

    // First parse
    let (deltas1, offset1) = parse_session_usage(&path, 0);
    assert_eq!(deltas1.input_tokens, 100);
    assert_eq!(deltas1.output_tokens, 50);

    // Append more data
    {
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, r#"{{"type":"assistant","message":{{"role":"assistant","model":"claude-sonnet-4-5-20250929","usage":{{"input_tokens":200,"output_tokens":75}},"stop_reason":"end_turn"}},"timestamp":"2026-01-01T00:00:02Z"}}"#).unwrap();
    }

    // Second parse from offset — only gets new data
    let (deltas2, offset2) = parse_session_usage(&path, offset1);
    assert_eq!(deltas2.input_tokens, 200);
    assert_eq!(deltas2.output_tokens, 75);
    assert!(offset2 > offset1);
}

#[test]
fn parse_session_usage_missing_file() {
    let (deltas, offset) = parse_session_usage(Path::new("/nonexistent/file.jsonl"), 0);
    assert_eq!(deltas.input_tokens, 0);
    assert_eq!(deltas.output_tokens, 0);
    assert_eq!(offset, 0);
}

#[test]
fn parse_session_usage_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.jsonl");
    fs::File::create(&path).unwrap();

    let (deltas, offset) = parse_session_usage(&path, 0);
    assert_eq!(deltas.input_tokens, 0);
    assert_eq!(offset, 0);
    assert!(deltas.model.is_none());
}

#[test]
fn parse_session_usage_skips_non_assistant_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session_log(
        dir.path(),
        &[
            r#"{"type":"user","message":{"role":"user","usage":{"input_tokens":999}}}"#,
            r#"{"type":"result","content":"ok","usage":{"input_tokens":888}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":42,"output_tokens":10}}}"#,
        ],
    );

    let (deltas, _) = parse_session_usage(&path, 0);
    assert_eq!(deltas.input_tokens, 42);
    assert_eq!(deltas.output_tokens, 10);
}

#[test]
fn parse_session_usage_handles_incomplete_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.jsonl");
    {
        let mut f = fs::File::create(&path).unwrap();
        // Complete line
        writeln!(f, r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":100,"output_tokens":50}}}}}}"#).unwrap();
        // Incomplete line (no newline)
        write!(
            f,
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":200}}}}}}"#
        )
        .unwrap();
    }

    let (deltas, offset) = parse_session_usage(&path, 0);
    // Should only process the complete line
    assert_eq!(deltas.input_tokens, 100);
    assert_eq!(deltas.output_tokens, 50);

    // Verify that appending the newline allows the second line to be read
    {
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f).unwrap();
    }
    let (deltas2, _) = parse_session_usage(&path, offset);
    assert_eq!(deltas2.input_tokens, 200);
}

#[test]
fn write_records_produces_valid_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let metrics_dir = dir.path().join("metrics");

    let collector = UsageMetricsCollector {
        state: Arc::new(Mutex::new(MaterializedState::default())),
        metrics_dir: metrics_dir.clone(),
        sessions: HashMap::new(),
        agent_meta: HashMap::new(),
        health: Arc::new(Mutex::new(MetricsHealth::default())),
        cycle_count: 0,
    };

    let records = vec![
        UsageRecord {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            agent_id: "agent-1".to_string(),
            session_id: "agent-1".to_string(),
            agent_kind: Some("builder".to_string()),
            job_id: Some("job-1".to_string()),
            job_kind: Some("build".to_string()),
            job_step: Some("plan".to_string()),
            namespace: Some("myproject".to_string()),
            status: "running".to_string(),
            input_tokens: 1000,
            output_tokens: 500,
            cache_creation_input_tokens: 100,
            cache_read_input_tokens: 200,
            model: Some("claude-sonnet-4-5-20250929".to_string()),
        },
        UsageRecord {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            agent_id: "agent-2".to_string(),
            session_id: "agent-2".to_string(),
            agent_kind: None,
            job_id: None,
            job_kind: None,
            job_step: None,
            namespace: None,
            status: "idle".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            model: None,
        },
    ];

    collector.write_records(&records).unwrap();

    let path = metrics_dir.join("usage.jsonl");
    let content = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2);

    // Each line should be valid JSON
    for line in &lines {
        let parsed: UsageRecord = serde_json::from_str(line).unwrap();
        assert!(!parsed.agent_id.is_empty());
    }

    // First record should have optional fields present
    let r1: UsageRecord = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(r1.agent_kind.as_deref(), Some("builder"));
    assert_eq!(r1.input_tokens, 1000);

    // Second record should skip None fields
    let r2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert!(r2.get("agent_kind").is_none());
    assert!(r2.get("model").is_none());
}

#[test]
fn rotation_shifts_files_and_writes_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let metrics_dir = dir.path().join("metrics");
    fs::create_dir_all(&metrics_dir).unwrap();

    let usage_path = metrics_dir.join("usage.jsonl");

    // Create a file that exceeds the size limit
    {
        let mut f = fs::File::create(&usage_path).unwrap();
        let dummy = "x".repeat(1024);
        // Write enough to exceed MAX_METRICS_SIZE (we'll override the check)
        for _ in 0..(MAX_METRICS_SIZE / 1024 + 1) {
            writeln!(f, "{}", dummy).unwrap();
        }
    }

    // Set up collector with one tracked session
    let mut sessions = HashMap::new();
    sessions.insert(
        "test-agent".to_string(),
        SessionParseState {
            offset: 0,
            input_tokens: 500,
            output_tokens: 250,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            model: Some("claude-sonnet-4-5-20250929".to_string()),
        },
    );

    let collector = UsageMetricsCollector {
        state: Arc::new(Mutex::new(MaterializedState::default())),
        metrics_dir: metrics_dir.clone(),
        sessions,
        agent_meta: HashMap::new(),
        health: Arc::new(Mutex::new(MetricsHealth::default())),
        cycle_count: 0,
    };

    collector.rotate_if_needed();

    // Old file should be rotated to .1
    assert!(metrics_dir.join("usage.jsonl.1").exists());

    // New file should exist with baseline
    assert!(usage_path.exists());
    let content = fs::read_to_string(&usage_path).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1);

    let record: UsageRecord = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(record.agent_id, "test-agent");
    assert_eq!(record.input_tokens, 500);
    assert_eq!(record.output_tokens, 250);
}

#[test]
fn iso_now_produces_valid_timestamp() {
    let ts = iso_now();
    assert!(ts.len() >= 20, "timestamp too short: {ts}");
    assert!(ts.ends_with('Z'));
    assert!(ts.contains('T'));

    // Should parse as a date
    let parts: Vec<&str> = ts.split('T').collect();
    assert_eq!(parts.len(), 2);
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    assert_eq!(date_parts.len(), 3);
    let year: u32 = date_parts[0].parse().unwrap();
    assert!(year >= 2025);
}
