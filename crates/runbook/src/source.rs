// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Bead-backed runbook source: materialize runbook files from beads at startup.
//!
//! When `OJ_BEAD_RUNBOOKS` is set, the daemon pre-materializes runbook content
//! from beads into a staging directory before the normal filesystem discovery
//! runs. This allows the existing `find_runbook_by_*` functions to work without
//! modification — they just see extra files.

use std::path::{Path, PathBuf};

/// A single bead that contains runbook content.
#[derive(Debug, Clone)]
pub struct BeadRunbook {
    /// The bead ID (e.g. "od-abc123").
    pub bead_id: String,
    /// Filename to write (e.g. "deploy.hcl").
    pub filename: String,
    /// Raw runbook file content.
    pub content: String,
}

/// Result of materializing bead runbooks to disk.
#[derive(Debug, Default)]
pub struct MaterializeResult {
    /// Number of runbook files successfully written.
    pub materialized: usize,
    /// Bead IDs that failed to materialize (with error messages).
    pub errors: Vec<(String, String)>,
}

/// Materialize bead runbooks into a target directory.
///
/// Each bead's content is written as a file under `target_dir`. The caller
/// should point this at the project's `.oj/runbooks/` directory (or a
/// shadow directory that is searched alongside it).
///
/// Existing files with the same name are NOT overwritten — filesystem
/// runbooks take precedence over bead-sourced ones.
///
/// Returns a summary of what was materialized.
pub fn materialize_bead_runbooks(
    beads: &[BeadRunbook],
    target_dir: &Path,
) -> MaterializeResult {
    let mut result = MaterializeResult::default();

    if beads.is_empty() {
        return result;
    }

    // Ensure target directory exists
    if let Err(e) = std::fs::create_dir_all(target_dir) {
        tracing::warn!(
            dir = %target_dir.display(),
            error = %e,
            "failed to create bead runbook staging directory"
        );
        for bead in beads {
            result.errors.push((bead.bead_id.clone(), e.to_string()));
        }
        return result;
    }

    for bead in beads {
        let dest = target_dir.join(&bead.filename);

        // Don't overwrite existing filesystem runbooks
        if dest.exists() {
            tracing::debug!(
                bead_id = %bead.bead_id,
                file = %bead.filename,
                "skipping bead runbook: filesystem file already exists"
            );
            continue;
        }

        match std::fs::write(&dest, &bead.content) {
            Ok(()) => {
                tracing::info!(
                    bead_id = %bead.bead_id,
                    file = %bead.filename,
                    "materialized bead runbook"
                );
                result.materialized += 1;
            }
            Err(e) => {
                tracing::warn!(
                    bead_id = %bead.bead_id,
                    file = %bead.filename,
                    error = %e,
                    "failed to materialize bead runbook"
                );
                result.errors.push((bead.bead_id.clone(), e.to_string()));
            }
        }
    }

    result
}

/// Build a mapping from materialized filenames to their source bead IDs.
///
/// Used by the daemon to annotate `RunbookLoaded` events with the correct
/// [`RunbookSource::Bead`] when the loaded runbook came from a bead.
pub fn bead_source_map(beads: &[BeadRunbook]) -> std::collections::HashMap<String, String> {
    beads
        .iter()
        .map(|b| (b.filename.clone(), b.bead_id.clone()))
        .collect()
}

/// Parse the `OJ_BEAD_RUNBOOKS` environment variable.
///
/// Format: `bead_id:filename:path` entries separated by `;`
/// where `path` points to a file containing the runbook content.
///
/// Example: `od-abc:deploy.hcl:/tmp/deploy.hcl;od-def:ci.toml:/tmp/ci.toml`
///
/// Returns an empty vec if the env var is not set.
pub fn load_bead_runbooks_from_env() -> Vec<BeadRunbook> {
    let val = match std::env::var("OJ_BEAD_RUNBOOKS") {
        Ok(v) if !v.is_empty() => v,
        _ => return Vec::new(),
    };

    let mut beads = Vec::new();
    for entry in val.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let parts: Vec<&str> = entry.splitn(3, ':').collect();
        if parts.len() != 3 {
            tracing::warn!(
                entry = entry,
                "invalid OJ_BEAD_RUNBOOKS entry (expected bead_id:filename:path)"
            );
            continue;
        }
        let (bead_id, filename, path) = (parts[0], parts[1], parts[2]);
        match std::fs::read_to_string(path) {
            Ok(content) => {
                beads.push(BeadRunbook {
                    bead_id: bead_id.to_string(),
                    filename: filename.to_string(),
                    content,
                });
            }
            Err(e) => {
                tracing::warn!(
                    bead_id = bead_id,
                    path = path,
                    error = %e,
                    "failed to read bead runbook content file"
                );
            }
        }
    }
    beads
}

/// Staging directory for materialized bead runbooks within the state dir.
pub fn bead_staging_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("bead-runbooks")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn materialize_writes_files() {
        let dir = tempfile::tempdir().unwrap();
        let beads = vec![
            BeadRunbook {
                bead_id: "od-1".to_string(),
                filename: "deploy.hcl".to_string(),
                content: "command \"deploy\" {}\n".to_string(),
            },
            BeadRunbook {
                bead_id: "od-2".to_string(),
                filename: "ci.toml".to_string(),
                content: "[commands.ci]\nrun = \"echo ci\"\n".to_string(),
            },
        ];

        let result = materialize_bead_runbooks(&beads, dir.path());
        assert_eq!(result.materialized, 2);
        assert!(result.errors.is_empty());

        assert_eq!(
            fs::read_to_string(dir.path().join("deploy.hcl")).unwrap(),
            "command \"deploy\" {}\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("ci.toml")).unwrap(),
            "[commands.ci]\nrun = \"echo ci\"\n"
        );
    }

    #[test]
    fn materialize_skips_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("deploy.hcl"), "original content").unwrap();

        let beads = vec![BeadRunbook {
            bead_id: "od-1".to_string(),
            filename: "deploy.hcl".to_string(),
            content: "bead content".to_string(),
        }];

        let result = materialize_bead_runbooks(&beads, dir.path());
        assert_eq!(result.materialized, 0);

        // Original file should be unchanged
        assert_eq!(
            fs::read_to_string(dir.path().join("deploy.hcl")).unwrap(),
            "original content"
        );
    }

    #[test]
    fn materialize_empty_beads_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let result = materialize_bead_runbooks(&[], dir.path());
        assert_eq!(result.materialized, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn bead_source_map_builds_mapping() {
        let beads = vec![
            BeadRunbook {
                bead_id: "od-1".to_string(),
                filename: "deploy.hcl".to_string(),
                content: String::new(),
            },
            BeadRunbook {
                bead_id: "od-2".to_string(),
                filename: "ci.toml".to_string(),
                content: String::new(),
            },
        ];

        let map = bead_source_map(&beads);
        assert_eq!(map.get("deploy.hcl"), Some(&"od-1".to_string()));
        assert_eq!(map.get("ci.toml"), Some(&"od-2".to_string()));
    }
}
