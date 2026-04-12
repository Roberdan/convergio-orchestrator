//! SpecComplianceGate — blocks task submit if deliverable doesn't match spec.
//!
//! Parses the task description for key requirements (crate deps, file creation,
//! route declarations) and verifies they exist in the workspace.
//! This prevents agents from declaring "done" when they built something different.

use rusqlite::Connection;

use crate::gates::GateError;

/// Check that task deliverable matches its spec before allowing submit.
/// Extracts requirements from task description and verifies against filesystem.
pub fn spec_compliance_gate(conn: &Connection, task_id: i64) -> Result<(), GateError> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT title, description FROM tasks WHERE id = ?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((title, description)) = row else {
        return Ok(()); // No task found — let other gates handle
    };
    let desc = description.unwrap_or_default();
    let full_text = format!("{title} {desc}");

    // Extract required crate dependencies mentioned in spec
    let required_deps = extract_required_deps(&full_text);
    for (crate_name, dep_name) in &required_deps {
        let cargo_path = format!("daemon/crates/{crate_name}/Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&cargo_path) {
            if !content.contains(dep_name) {
                return Err(GateError {
                    gate: "SpecComplianceGate",
                    reason: format!(
                        "spec requires '{dep_name}' in {crate_name}/Cargo.toml but it's missing"
                    ),
                    expected: format!(
                        "add '{dep_name}' as a dependency in {crate_name}/Cargo.toml"
                    ),
                });
            }
        }
    }

    // Extract required files that must exist
    for path in extract_required_files(&full_text) {
        if !std::path::Path::new(&path).exists() {
            return Err(GateError {
                gate: "SpecComplianceGate",
                reason: format!("spec requires file '{path}' but it doesn't exist"),
                expected: format!("create the file at '{path}' as described in the task spec"),
            });
        }
    }

    Ok(())
}

/// Extract (crate_name, dependency_name) pairs from spec text.
/// Looks for patterns like "LanceDB" near "convergio-knowledge".
fn extract_required_deps(text: &str) -> Vec<(String, String)> {
    let mut deps = Vec::new();
    let dep_keywords = [
        ("lancedb", "lancedb"),
        ("fastembed", "fastembed"),
        ("reqwest", "reqwest"),
        ("qdrant", "qdrant"),
    ];
    // Find crate names in text
    let crate_names: Vec<&str> = text
        .split_whitespace()
        .filter(|w| w.starts_with("convergio-") && !w.contains('/'))
        .collect();

    let text_lower = text.to_lowercase();
    for (keyword, dep) in &dep_keywords {
        if text_lower.contains(keyword) {
            for crate_name in &crate_names {
                deps.push((crate_name.to_string(), dep.to_string()));
            }
        }
    }
    deps
}

/// Extract file paths that the spec says must be created.
fn extract_required_files(text: &str) -> Vec<String> {
    let mut files = Vec::new();
    let markers = ["create file", "create:", "new file:"];
    let text_lower = text.to_lowercase();
    for marker in &markers {
        for (idx, _) in text_lower.match_indices(marker) {
            let rest = &text[idx + marker.len()..];
            let path: String = rest
                .trim()
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ',' && *c != ';')
                .collect();
            if path.contains('/') && !path.is_empty() {
                files.push(path);
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_lancedb_dep() {
        let deps =
            extract_required_deps("Implement LanceDB vector store in convergio-knowledge crate");
        assert!(!deps.is_empty());
        assert!(deps.iter().any(|(_, d)| d == "lancedb"));
    }

    #[test]
    fn no_deps_for_plain_text() {
        let deps = extract_required_deps("fix the rate limiter bug");
        assert!(deps.is_empty());
    }
}
