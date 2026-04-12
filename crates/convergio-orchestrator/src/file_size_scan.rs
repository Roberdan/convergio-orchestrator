//! File size scan — pre-commit check for the 300-line rule.
//!
//! POST /api/lint/file-size-check
//! Scans a directory for code files exceeding the line limit.
//! Agents call this before committing to catch ALL violations at once.

use std::path::Path;
use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use axum::routing::post;
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

const MAX_LINES: usize = 300;
const CODE_EXTENSIONS: &[&str] = &["rs", "ts", "js", "sh"];

#[derive(Debug, Deserialize)]
struct ScanRequest {
    /// Path to scan (worktree root). Defaults to current directory.
    #[serde(default = "default_path")]
    path: String,
}

fn default_path() -> String {
    ".".into()
}

#[derive(Debug, serde::Serialize)]
struct Violation {
    file: String,
    lines: usize,
    over_by: usize,
}

pub fn lint_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/lint/file-size-check", post(handle_size_check))
        .with_state(state)
}

async fn handle_size_check(
    State(_state): State<Arc<PlanState>>,
    Json(req): Json<ScanRequest>,
) -> Json<serde_json::Value> {
    let root = Path::new(&req.path);
    // Block path traversal: reject absolute paths and `..` components
    if root.is_absolute() || req.path.contains("..") {
        return Json(json!({"error": "path must be relative and cannot contain '..'"}));
    }
    if !root.exists() {
        return Json(json!({"error": format!("path '{}' not found", req.path)}));
    }
    let exclude = load_g2_exclude(root);
    let mut violations = Vec::new();
    scan_dir(root, root, &exclude, &mut violations);
    let pass = violations.is_empty();
    Json(json!({
        "pass": pass,
        "max_lines": MAX_LINES,
        "violations": violations,
        "scanned_path": req.path,
    }))
}

fn scan_dir(root: &Path, dir: &Path, exclude: &[String], violations: &mut Vec<Violation>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            scan_dir(root, &path, exclude, violations);
        } else if is_code_file(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().to_string();
            if is_excluded(&rel_str, exclude) {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                let lines = content.lines().count();
                if lines > MAX_LINES {
                    violations.push(Violation {
                        file: rel_str,
                        lines,
                        over_by: lines - MAX_LINES,
                    });
                }
            }
        }
    }
}

fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| CODE_EXTENSIONS.contains(&ext))
}

fn is_excluded(file: &str, patterns: &[String]) -> bool {
    for pattern in patterns {
        if pattern.starts_with('#') || pattern.is_empty() {
            continue;
        }
        if file.contains(pattern.trim_matches('*')) {
            return true;
        }
    }
    false
}

fn load_g2_exclude(root: &Path) -> Vec<String> {
    let path = root.join(".g2-exclude");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| l.to_string())
        .collect()
}
