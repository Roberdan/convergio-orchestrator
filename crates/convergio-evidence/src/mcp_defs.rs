//! MCP tool definitions for the evidence extension.

use convergio_types::extension::McpToolDef;
use serde_json::json;

pub fn evidence_tools() -> Vec<McpToolDef> {
    vec![
        McpToolDef {
            name: "cvg_record_evidence".into(),
            description: "Record evidence for a task (test result, output, etc). \
                          Required before task can be submitted."
                .into(),
            method: "POST".into(),
            path: "/api/plan-db/task/evidence".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_db_id": {"type": "integer", "description": "Task database ID"},
                    "evidence_type": {"type": "string", "description": "test_result, test_pass, commit, pr_url"},
                    "command": {"type": "string", "description": "Command that produced the evidence"},
                    "output_summary": {"type": "string", "description": "Summary of the output"},
                    "exit_code": {"type": "integer", "description": "Exit code (0 = success)"}
                },
                "required": ["task_db_id", "evidence_type"]
            }),
            min_ring: "trusted".into(),
            path_params: vec![],
        },
        McpToolDef {
            name: "cvg_evidence_preflight".into(),
            description: "Pre-check what evidence a task still needs before submission.".into(),
            method: "GET".into(),
            path: "/api/evidence/preflight/:task_id".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"task_id": {"type": "integer"}},
                "required": ["task_id"]
            }),
            min_ring: "community".into(),
            path_params: vec!["task_id".into()],
        },
    ]
}
