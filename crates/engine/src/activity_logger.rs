// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Unified append-only logger for per-entity activity logs.
//!
//! Provides a single parameterized type `ActivityLogger<K>` that handles
//! job, worker, and queue logging with type-specific behavior determined
//! by the `LogKind` marker trait.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use oj_core::ShortId;

use crate::log_paths;
use crate::time_fmt::format_utc_now;

/// Marker trait for activity log kinds.
///
/// Implementations define the subdirectory and tracing field name for each log type.
pub trait LogKind {
    /// Subdirectory within logs dir (e.g., "job", "worker", "queue").
    const SUBDIR: &'static str;
}

/// Marker type for job logs.
pub struct JobLog;
impl LogKind for JobLog {
    const SUBDIR: &'static str = "job";
}

/// Marker type for worker logs.
pub struct WorkerLog;
impl LogKind for WorkerLog {
    const SUBDIR: &'static str = "worker";
}

/// Marker type for queue logs.
pub struct QueueLog;
impl LogKind for QueueLog {
    const SUBDIR: &'static str = "queue";
}

/// Unified append-only logger for per-entity activity logs.
///
/// Writes human-readable timestamped lines to:
///   `<log_dir>/<subdir>/<entity_id>.log`
///
/// Each `append()` call opens, writes, and closes the file.
/// This is safe for the low write frequency of activity events.
pub struct ActivityLogger<K: LogKind> {
    log_dir: PathBuf,
    _kind: PhantomData<K>,
}

impl<K: LogKind> ActivityLogger<K> {
    /// Create a new activity logger.
    pub fn new(log_dir: PathBuf) -> Self {
        Self {
            log_dir,
            _kind: PhantomData,
        }
    }

    /// Write a timestamped line to a log file.
    ///
    /// Format: `YYYY-MM-DDTHH:MM:SSZ [{label}] {message}`
    fn write_line(&self, path: &Path, label: &str, message: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let ts = format_utc_now();
        writeln!(file, "{} [{}] {}", ts, label, message)?;
        Ok(())
    }
}

// =============================================================================
// JobLogger implementation
// =============================================================================

/// Type alias for job activity logger.
pub type JobLogger = ActivityLogger<JobLog>;

impl ActivityLogger<JobLog> {
    /// Returns the base log directory path.
    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// Append a log line for the given job.
    ///
    /// Format: `2026-01-30T08:14:09Z [step] message`
    ///
    /// Failures are logged via tracing but do not propagate — logging
    /// must not break the engine.
    pub fn append(&self, job_id: &str, step: &str, message: &str) {
        let path = log_paths::job_log_path(&self.log_dir, job_id);
        if let Err(e) = self.write_line(&path, step, message) {
            tracing::warn!(
                job_id,
                error = %e,
                "failed to write job log"
            );
        }
    }

    /// Append a pointer line to the agent log for a step.
    ///
    /// Format: `2026-01-30T08:17:00Z [step] agent log: /full/path/to/logs/agent/<agent_id>.log`
    pub fn append_agent_pointer(&self, job_id: &str, step: &str, agent_id: &str) {
        let log_path = log_paths::agent_log_path(&self.log_dir, agent_id);
        let message = format!("agent log: {}", log_path.display());
        self.append(job_id, step, &message);
    }

    /// Copy the agent's session.jsonl to the logs directory.
    ///
    /// Copies the source file to `{logs_dir}/agent/{agent_id}/session.jsonl`.
    /// Failures are logged via tracing but do not propagate.
    pub fn copy_session_log(&self, agent_id: &str, source: &Path) {
        let dest_dir = log_paths::agent_session_log_dir(&self.log_dir, agent_id);
        let dest = dest_dir.join("session.jsonl");

        if let Err(e) = fs::create_dir_all(&dest_dir) {
            tracing::warn!(
                agent_id,
                error = %e,
                "failed to create session log directory"
            );
            return;
        }

        if let Err(e) = fs::copy(source, &dest) {
            tracing::warn!(
                agent_id,
                source = %source.display(),
                dest = %dest.display(),
                error = %e,
                "failed to copy session log"
            );
        } else {
            tracing::debug!(
                agent_id,
                dest = %dest.display(),
                "copied session log"
            );
        }
    }

    /// Append a fenced block to the job log.
    ///
    /// Format:
    /// ```text
    /// {timestamp} [{step}] ```{label}
    /// {content}
    /// {timestamp} [{step}] ```
    /// ```
    pub fn append_fenced(&self, job_id: &str, step: &str, label: &str, content: &str) {
        let path = log_paths::job_log_path(&self.log_dir, job_id);
        if let Err(e) = self.write_fenced(&path, step, label, content) {
            tracing::warn!(
                job_id,
                error = %e,
                "failed to write job log"
            );
        }
    }

    fn write_fenced(
        &self,
        path: &Path,
        step: &str,
        label: &str,
        content: &str,
    ) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let ts = format_utc_now();
        writeln!(file, "{} [{}] ```{}", ts, step, label)?;
        write!(file, "{}", content)?;
        if !content.ends_with('\n') {
            writeln!(file)?;
        }
        let ts = format_utc_now();
        writeln!(file, "{} [{}] ```", ts, step)?;
        Ok(())
    }

    /// Append a spawn error to an agent's log file.
    ///
    /// Format: `2026-01-30T08:14:09Z error: <message>`
    ///
    /// This is used when agent spawn fails before the watcher is started,
    /// so there's no other mechanism to write to the agent log.
    pub fn append_agent_error(&self, agent_id: &str, message: &str) {
        let path = log_paths::agent_log_path(&self.log_dir, agent_id);
        if let Err(e) = self.write_agent_error(&path, message) {
            tracing::warn!(
                agent_id,
                error = %e,
                "failed to write agent spawn error log"
            );
        }
    }

    fn write_agent_error(&self, path: &Path, message: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let ts = format_utc_now();
        writeln!(file, "{} error: {}", ts, message)?;
        Ok(())
    }
}

// =============================================================================
// WorkerLogger implementation
// =============================================================================

/// Type alias for worker activity logger.
pub type WorkerLogger = ActivityLogger<WorkerLog>;

impl ActivityLogger<WorkerLog> {
    /// Append a log line for the given worker.
    ///
    /// Format: `2026-01-30T08:14:09Z [worker] message`
    ///
    /// Failures are logged via tracing but do not propagate — logging
    /// must not break the engine.
    pub fn append(&self, worker_name: &str, message: &str) {
        let path = log_paths::worker_log_path(&self.log_dir, worker_name);
        if let Err(e) = self.write_line(&path, "worker", message) {
            tracing::warn!(
                worker_name,
                error = %e,
                "failed to write worker log"
            );
        }
    }
}

// =============================================================================
// QueueLogger implementation
// =============================================================================

/// Type alias for queue activity logger.
pub type QueueLogger = ActivityLogger<QueueLog>;

impl ActivityLogger<QueueLog> {
    /// Append a timestamped log line to the queue's log file.
    ///
    /// Format: `2026-01-30T08:14:09Z [item_id_prefix] message`
    ///
    /// Failures are logged via tracing but do not propagate — logging
    /// must not break the engine.
    pub fn append(&self, queue_name: &str, item_id: &str, message: &str) {
        let path = log_paths::queue_log_path(&self.log_dir, queue_name);
        let prefix = item_id.short(8);
        if let Err(e) = self.write_line(&path, prefix, message) {
            tracing::warn!(
                queue_name,
                error = %e,
                "failed to write queue log"
            );
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod job_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn append_creates_directory_and_file() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        let logger = JobLogger::new(log_dir.clone());

        logger.append("pipe-1", "init", "job created");

        let content = std::fs::read_to_string(log_dir.join("job/pipe-1.log")).unwrap();
        assert!(content.contains("[init] job created"));
    }

    #[test]
    fn multiple_appends_produce_ordered_lines() {
        let dir = tempdir().unwrap();
        let logger = JobLogger::new(dir.path().to_path_buf());

        logger.append("pipe-1", "init", "step started");
        logger.append("pipe-1", "init", "shell: echo hello");
        logger.append("pipe-1", "init", "shell completed (exit 0)");

        let content = std::fs::read_to_string(dir.path().join("job/pipe-1.log")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("[init] step started"));
        assert!(lines[1].contains("[init] shell: echo hello"));
        assert!(lines[2].contains("[init] shell completed (exit 0)"));
    }

    #[test]
    fn lines_match_expected_format() {
        let dir = tempdir().unwrap();
        let logger = JobLogger::new(dir.path().to_path_buf());

        logger.append("pipe-1", "plan", "agent spawned: planner");

        let content = std::fs::read_to_string(dir.path().join("job/pipe-1.log")).unwrap();
        let line = content.trim();

        // Format: YYYY-MM-DDTHH:MM:SSZ [step] message
        assert!(
            line.chars().nth(4) == Some('-'),
            "expected date format, got: {}",
            line
        );
        assert!(line.contains("[plan] agent spawned: planner"));
        assert_eq!(line.chars().nth(10), Some('T'));
        assert!(line.ends_with("Z [plan] agent spawned: planner"));
        assert!(
            line.len() > 20,
            "line too short for expected format: {}",
            line
        );
    }

    #[test]
    fn separate_jobs_get_separate_files() {
        let dir = tempdir().unwrap();
        let logger = JobLogger::new(dir.path().to_path_buf());

        logger.append("pipe-1", "init", "first job");
        logger.append("pipe-2", "init", "second job");

        let content1 = std::fs::read_to_string(dir.path().join("job/pipe-1.log")).unwrap();
        let content2 = std::fs::read_to_string(dir.path().join("job/pipe-2.log")).unwrap();
        assert!(content1.contains("first job"));
        assert!(content2.contains("second job"));
        assert!(!content1.contains("second job"));
    }

    #[test]
    fn bad_path_does_not_panic() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("blocker");
        std::fs::write(&file_path, "not a dir").unwrap();

        let logger = JobLogger::new(file_path.join("nested"));

        // Should not panic, just log a warning
        logger.append("pipe-1", "init", "should not panic");
    }

    #[test]
    fn agent_pointer_uses_absolute_path() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        let logger = JobLogger::new(log_dir.clone());

        let agent_id = "8cf5e1df-a434-4029-a369-c95af9c374c9";
        logger.append_agent_pointer("pipe-1", "plan", agent_id);

        let content = std::fs::read_to_string(log_dir.join("job/pipe-1.log")).unwrap();
        let expected_path = log_dir.join("agent").join(format!("{}.log", agent_id));
        assert!(
            content.contains(&expected_path.display().to_string()),
            "expected absolute path in log, got: {}",
            content
        );
    }

    #[test]
    fn copy_session_log_creates_directory_and_copies_file() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        let logger = JobLogger::new(log_dir.clone());

        let source_dir = dir.path().join("source");
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("session.jsonl");
        std::fs::write(&source, r#"{"type":"user","message":"hello"}"#).unwrap();

        let agent_id = "8cf5e1df-a434-4029-a369-c95af9c374c9";
        logger.copy_session_log(agent_id, &source);

        let dest = log_dir.join("agent").join(agent_id).join("session.jsonl");
        assert!(dest.exists(), "session.jsonl should exist at {:?}", dest);

        let content = std::fs::read_to_string(&dest).unwrap();
        assert!(content.contains(r#"{"type":"user","message":"hello"}"#));
    }

    #[test]
    fn append_agent_error_writes_to_agent_log() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        let logger = JobLogger::new(log_dir.clone());

        let agent_id = "8cf5e1df-a434-4029-a369-c95af9c374c9";
        logger.append_agent_error(agent_id, "rate limit exceeded");

        let content =
            std::fs::read_to_string(log_dir.join("agent").join(format!("{}.log", agent_id)))
                .unwrap();
        assert!(
            content.contains("error: rate limit exceeded"),
            "expected error in agent log, got: {}",
            content
        );
        assert!(content.starts_with("20"), "expected timestamp prefix");
    }

    #[test]
    fn append_agent_error_appends_multiple() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        let logger = JobLogger::new(log_dir.clone());

        let agent_id = "test-agent-1";
        logger.append_agent_error(agent_id, "first error");
        logger.append_agent_error(agent_id, "second error");

        let content =
            std::fs::read_to_string(log_dir.join("agent").join(format!("{}.log", agent_id)))
                .unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("error: first error"));
        assert!(lines[1].contains("error: second error"));
    }

    #[test]
    fn copy_session_log_handles_missing_source() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        let logger = JobLogger::new(log_dir.clone());

        let source = dir.path().join("nonexistent.jsonl");

        let agent_id = "abc-123";
        // Should not panic, just log a warning
        logger.copy_session_log(agent_id, &source);

        let dest_dir = log_dir.join("agent").join(agent_id);
        assert!(dest_dir.exists());
    }

    #[test]
    fn append_fenced_writes_correctly_formatted_block() {
        let dir = tempdir().unwrap();
        let logger = JobLogger::new(dir.path().to_path_buf());

        logger.append_fenced("pipe-1", "init", "stdout", "hello world\n");

        let content = std::fs::read_to_string(dir.path().join("job/pipe-1.log")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("[init] ```stdout"));
        assert_eq!(lines[1], "hello world");
        assert!(lines[2].contains("[init] ```"));
        assert!(!lines[2].contains("```stdout"));
    }

    #[test]
    fn append_fenced_adds_trailing_newline_when_missing() {
        let dir = tempdir().unwrap();
        let logger = JobLogger::new(dir.path().to_path_buf());

        logger.append_fenced("pipe-1", "build", "stderr", "warning: unused variable");

        let content = std::fs::read_to_string(dir.path().join("job/pipe-1.log")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("[build] ```stderr"));
        assert_eq!(lines[1], "warning: unused variable");
        assert!(lines[2].contains("[build] ```"));
    }

    #[test]
    fn append_fenced_multiline_content() {
        let dir = tempdir().unwrap();
        let logger = JobLogger::new(dir.path().to_path_buf());

        logger.append_fenced(
            "pipe-1",
            "build",
            "stdout",
            "Compiling oj v0.1.0\n    Finished dev target(s) in 12.34s\n",
        );

        let content = std::fs::read_to_string(dir.path().join("job/pipe-1.log")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("[build] ```stdout"));
        assert_eq!(lines[1], "Compiling oj v0.1.0");
        assert_eq!(lines[2], "    Finished dev target(s) in 12.34s");
        assert!(lines[3].contains("[build] ```"));
    }

    #[test]
    fn append_fenced_integrates_with_append() {
        let dir = tempdir().unwrap();
        let logger = JobLogger::new(dir.path().to_path_buf());

        logger.append_fenced("pipe-1", "init", "stdout", "hello world\n");
        logger.append("pipe-1", "init", "shell completed (exit 0)");

        let content = std::fs::read_to_string(dir.path().join("job/pipe-1.log")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("[init] ```stdout"));
        assert_eq!(lines[1], "hello world");
        assert!(lines[2].contains("[init] ```"));
        assert!(lines[3].contains("[init] shell completed (exit 0)"));
    }
}

#[cfg(test)]
mod worker_tests {
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
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, QueueLogger) {
        let dir = TempDir::new().unwrap();
        let logger = QueueLogger::new(dir.path().to_path_buf());
        (dir, logger)
    }

    #[test]
    fn creates_log_file_on_first_append() {
        let (dir, logger) = setup();
        logger.append(
            "build-queue",
            "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
            "pushed data={url=https://example.com}",
        );

        let path = dir.path().join("queue/build-queue.log");
        assert!(path.exists(), "log file should be created");

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[a1b2c3d4]"));
        assert!(content.contains("pushed data={url=https://example.com}"));
    }

    #[test]
    fn appends_multiple_entries() {
        let (dir, logger) = setup();
        let item_id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
        logger.append("q", item_id, "pushed");
        logger.append("q", item_id, "dispatched worker=my-worker");
        logger.append("q", item_id, "completed");

        let path = dir.path().join("queue/q.log");
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("pushed"));
        assert!(lines[1].contains("dispatched worker=my-worker"));
        assert!(lines[2].contains("completed"));
    }

    #[test]
    fn handles_namespaced_queue_name() {
        let (dir, logger) = setup();
        logger.append(
            "myproject/build-queue",
            "abcdef01-2345-6789-abcd-ef0123456789",
            "pushed",
        );

        let path = dir.path().join("queue/myproject/build-queue.log");
        assert!(path.exists(), "namespaced log file should be created");
    }

    #[test]
    fn truncates_item_id_prefix() {
        let (dir, logger) = setup();
        logger.append("q", "abcdef0123456789", "pushed");

        let path = dir.path().join("queue/q.log");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[abcdef01]"));
    }

    #[test]
    fn handles_short_item_id() {
        let (dir, logger) = setup();
        logger.append("q", "abc", "pushed");

        let path = dir.path().join("queue/q.log");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[abc]"));
    }

    #[test]
    fn log_line_format() {
        let (dir, logger) = setup();
        logger.append("q", "a1b2c3d4-full-id", "failed error=\"timeout exceeded\"");

        let path = dir.path().join("queue/q.log");
        let content = std::fs::read_to_string(&path).unwrap();
        let line = content.lines().next().unwrap();

        // Format: YYYY-MM-DDTHH:MM:SSZ [prefix] message
        assert!(line.ends_with("[a1b2c3d4] failed error=\"timeout exceeded\""));
        assert!(
            line.starts_with("20"),
            "line should start with timestamp: {}",
            line
        );
        assert!(line.contains('T'));
        assert!(line.contains('Z'));
    }
}
