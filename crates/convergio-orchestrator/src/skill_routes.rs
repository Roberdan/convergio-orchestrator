//! Skill prompt serving — returns `.claude/commands/{name}.md` content via API.
//!
//! Route: GET /api/skills/:name/prompt

use axum::extract::Path;
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use serde_json::json;

/// Build skill routes (stateless — reads from disk).
pub fn skill_routes() -> Router {
    Router::new().route("/api/skills/:name/prompt", get(handle_skill_prompt))
}

/// Serve a skill prompt by name.
///
/// Reads `.claude/commands/{name}.md` relative to the repo root.
/// Returns 404 if the skill file does not exist.
#[tracing::instrument(skip_all, fields(skill_name = %name))]
async fn handle_skill_prompt(Path(name): Path<String>) -> Json<serde_json::Value> {
    // Sanitize: only allow alphanumeric, dash, underscore
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Json(json!({"error": "invalid skill name"}));
    }

    let path = skill_file_path(&name);

    match std::fs::read_to_string(&path) {
        Ok(content) => Json(json!({
            "name": name,
            "content": content,
            "path": path.to_string_lossy(),
        })),
        Err(_) => Json(json!({
            "error": format!("skill '{}' not found", name),
            "searched": path.to_string_lossy(),
        })),
    }
}

/// Build the filesystem path for a skill file.
fn skill_file_path(name: &str) -> std::path::PathBuf {
    // Try repo-relative path first (daemon runs from repo root)
    let candidates = [
        std::path::PathBuf::from(".claude/commands").join(format!("{name}.md")),
        std::env::current_dir()
            .unwrap_or_default()
            .join(".claude/commands")
            .join(format!("{name}.md")),
    ];
    for p in &candidates {
        if p.exists() {
            return p.clone();
        }
    }
    // Return first candidate (caller handles not-found)
    candidates[0].clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_file_path_builds_correct_path() {
        let p = skill_file_path("check");
        assert!(p.to_string_lossy().contains("check.md"));
    }

    #[test]
    fn sanitize_rejects_path_traversal() {
        let name = "../etc/passwd";
        assert!(!name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
    }
}
