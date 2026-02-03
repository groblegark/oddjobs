// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

#[test]
fn append_creates_log_file_and_writes_line() {
    let dir = tempfile::tempdir().unwrap();
    let logger = WorkerLogger::new(dir.path().to_path_buf());

    logger.append("test-worker", "started (queue=bugs, concurrency=2)");

    let log_path = dir.path().join("worker/test-worker.log");
    assert!(log_path.exists());

    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("[worker] started (queue=bugs, concurrency=2)"));
    assert!(content.ends_with('\n'));
    // Check timestamp format: YYYY-MM-DDTHH:MM:SSZ
    let first_line = content.lines().next().unwrap();
    assert!(first_line.len() > 20);
    assert!(first_line.contains('T'));
    assert!(first_line.contains('Z'));
}

#[test]
fn append_accumulates_multiple_lines() {
    let dir = tempfile::tempdir().unwrap();
    let logger = WorkerLogger::new(dir.path().to_path_buf());

    logger.append("my-worker", "started");
    logger.append("my-worker", "dispatched item abc123");
    logger.append("my-worker", "stopped");

    let log_path = dir.path().join("worker/my-worker.log");
    let content = std::fs::read_to_string(&log_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("started"));
    assert!(lines[1].contains("dispatched item abc123"));
    assert!(lines[2].contains("stopped"));
}

#[test]
fn append_with_namespace_creates_subdirectory() {
    let dir = tempfile::tempdir().unwrap();
    let logger = WorkerLogger::new(dir.path().to_path_buf());

    logger.append("myproject/test-worker", "started");

    let log_path = dir.path().join("worker/myproject/test-worker.log");
    assert!(log_path.exists());

    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("[worker] started"));
}
