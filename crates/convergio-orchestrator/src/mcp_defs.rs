//! MCP tool definitions for the orchestrator extension.
//!
//! These are discovered at runtime via `/api/meta/mcp-tools`.

use convergio_types::extension::McpToolDef;
use serde_json::json;

pub fn orchestrator_tools() -> Vec<McpToolDef> {
    vec![
        McpToolDef {
            name: "cvg_list_plans".into(),
            description: "List plans. Defaults to active+paused (not all). Use status=all to see everything.".into(),
            method: "GET".into(),
            path: "/api/plan-db/list".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "status": {"type": "string", "description": "Filter: active, in_progress, paused, done, cancelled, failed, all (default: omit for active+paused only)"}
                }
            }),
            min_ring: "sandboxed".into(),
            path_params: vec![],
        },
        McpToolDef {
            name: "cvg_get_plan".into(),
            description: "Get full plan details by ID.".into(),
            method: "GET".into(),
            path: "/api/plan-db/json/:plan_id".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"plan_id": {"type": "integer"}},
                "required": ["plan_id"]
            }),
            min_ring: "community".into(),
            path_params: vec!["plan_id".into()],
        },
        McpToolDef {
            name: "cvg_get_execution_tree".into(),
            description: "Get execution tree for a plan with waves and tasks.".into(),
            method: "GET".into(),
            path: "/api/plan-db/execution-tree/:plan_id".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"plan_id": {"type": "integer"}},
                "required": ["plan_id"]
            }),
            min_ring: "community".into(),
            path_params: vec!["plan_id".into()],
        },
        McpToolDef {
            name: "cvg_update_task".into(),
            description: "Update task status or notes. Status: pending, in_progress, submitted."
                .into(),
            method: "POST".into(),
            path: "/api/plan-db/task/update".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "integer"},
                    "status": {"type": "string", "enum": ["pending","in_progress","submitted"]},
                    "agent_id": {"type": "string"},
                    "notes": {"type": "string"},
                    "summary": {"type": "string"}
                },
                "required": ["task_id", "status", "agent_id"]
            }),
            min_ring: "trusted".into(),
            path_params: vec![],
        },
        McpToolDef {
            name: "cvg_complete_task".into(),
            description: "Atomically complete a task: set notes, record evidence, and submit."
                .into(),
            method: "POST".into(),
            path: "/api/plan-db/task/complete-flow".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_db_id": {"type": "integer"},
                    "agent_id": {"type": "string"},
                    "pr_url": {"type": "string"},
                    "test_command": {"type": "string"},
                    "test_output": {"type": "string"},
                    "test_exit_code": {"type": "integer"},
                    "notes": {"type": "string"}
                },
                "required": ["task_db_id", "agent_id", "pr_url"]
            }),
            min_ring: "trusted".into(),
            path_params: vec![],
        },
        McpToolDef {
            name: "cvg_validate_plan".into(),
            description: "Run Thor validation for a plan. All wave tasks must be submitted first."
                .into(),
            method: "POST".into(),
            path: "/api/plan-db/validate".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"plan_id": {"type": "integer"}},
                "required": ["plan_id"]
            }),
            min_ring: "trusted".into(),
            path_params: vec![],
        },
        McpToolDef {
            name: "cvg_review_plan".into(),
            description: "Run pre-execution review for a plan.".into(),
            method: "POST".into(),
            path: "/api/plan-db/review".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"plan_id": {"type": "integer"}},
                "required": ["plan_id"]
            }),
            min_ring: "trusted".into(),
            path_params: vec![],
        },
        McpToolDef {
            name: "cvg_checkpoint_save".into(),
            description: "Save a checkpoint for a plan.".into(),
            method: "POST".into(),
            path: "/api/plan-db/checkpoint/save".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"plan_id": {"type": "integer"}},
                "required": ["plan_id"]
            }),
            min_ring: "trusted".into(),
            path_params: vec![],
        },
        McpToolDef {
            name: "cvg_plan_readiness".into(),
            description: "Check plan readiness: validates tasks, deps, test_criteria.".into(),
            method: "GET".into(),
            path: "/api/plan-db/readiness/:plan_id".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"plan_id": {"type": "integer"}},
                "required": ["plan_id"]
            }),
            min_ring: "community".into(),
            path_params: vec!["plan_id".into()],
        },
        McpToolDef {
            name: "cvg_plan_import".into(),
            description: "Import tasks from a YAML spec into an existing plan.".into(),
            method: "POST".into(),
            path: "/api/plan-db/import".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "plan_id": {"type": "integer"},
                    "spec": {"type": "string", "description": "YAML spec content"},
                    "import_mode": {"type": "string", "enum": ["append", "replace", "merge"], "description": "Import mode (default: append)"}
                },
                "required": ["plan_id", "spec"]
            }),
            min_ring: "trusted".into(),
            path_params: vec![],
        },
        McpToolDef {
            name: "cvg_update_task_content".into(),
            description: "Update task content fields (title, description, model, effort). Only on draft/todo plans.".into(),
            method: "PATCH".into(),
            path: "/api/plan-db/task/:task_id".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "integer", "description": "Task DB ID"},
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "executor_agent": {"type": "string"},
                    "model": {"type": "string"},
                    "effort_level": {"type": "integer"},
                    "test_criteria": {"type": "string"},
                    "verify": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["task_id"]
            }),
            min_ring: "trusted".into(),
            path_params: vec!["task_id".into()],
        },
        McpToolDef {
            name: "cvg_register_review".into(),
            description: "Register an external review verdict for a plan.".into(),
            method: "POST".into(),
            path: "/api/plan-db/review/register".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "plan_id": {"type": "integer"},
                    "reviewer_agent": {"type": "string"},
                    "verdict": {"type": "string", "enum": ["proceed", "revise", "reject"]},
                    "suggestions": {"type": "string"}
                },
                "required": ["plan_id", "reviewer_agent", "verdict"]
            }),
            min_ring: "trusted".into(),
            path_params: vec![],
        },
        McpToolDef {
            name: "cvg_start_plan".into(),
            description: "Start executing a plan (changes status from todo to in_progress).".into(),
            method: "POST".into(),
            path: "/api/plan-db/start/:plan_id".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "plan_id": {"type": "integer", "description": "Plan ID to start"}
                },
                "required": ["plan_id"]
            }),
            min_ring: "trusted".into(),
            path_params: vec!["plan_id".into()],
        },
        McpToolDef {
            name: "cvg_cancel_plan".into(),
            description: "Cancel a plan.".into(),
            method: "POST".into(),
            path: "/api/plan-db/cancel/:plan_id".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "plan_id": {"type": "integer", "description": "Plan ID to cancel"}
                },
                "required": ["plan_id"]
            }),
            min_ring: "trusted".into(),
            path_params: vec!["plan_id".into()],
        },
        McpToolDef {
            name: "cvg_resume_plan".into(),
            description: "Resume a paused or stale plan.".into(),
            method: "POST".into(),
            path: "/api/plan-db/resume/:plan_id".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "plan_id": {"type": "integer", "description": "Plan ID to resume"}
                },
                "required": ["plan_id"]
            }),
            min_ring: "trusted".into(),
            path_params: vec!["plan_id".into()],
        },
        McpToolDef {
            name: "cvg_wave_complete".into(),
            description: "Batch-complete all tasks in a wave: set notes, record evidence, submit. One call instead of N.".into(),
            method: "POST".into(),
            path: "/api/plan-db/wave/complete".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "wave_db_id": {"type": "integer", "description": "Database ID of the wave"},
                    "agent_id": {"type": "string"},
                    "pr_url": {"type": "string", "description": "PR URL shared by all tasks in the wave"},
                    "test_command": {"type": "string"},
                    "test_output": {"type": "string"},
                    "test_exit_code": {"type": "integer"}
                },
                "required": ["wave_db_id", "agent_id", "pr_url"]
            }),
            min_ring: "trusted".into(),
            path_params: vec![],
        },
        McpToolDef {
            name: "cvg_force_resume_plan".into(),
            description: "Force-resume a stuck plan: set in_progress, clear locks, reset stuck tasks.".into(),
            method: "POST".into(),
            path: "/api/plan-db/force-resume/:plan_id".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "plan_id": {"type": "integer", "description": "Plan ID to force-resume"}
                },
                "required": ["plan_id"]
            }),
            min_ring: "trusted".into(),
            path_params: vec!["plan_id".into()],
        },
        McpToolDef {
            name: "cvg_force_complete_plan".into(),
            description: "Force-complete a stuck plan: mark all tasks done. Use when code is merged but DB is stuck.".into(),
            method: "POST".into(),
            path: "/api/plan-db/force-complete/:plan_id".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "plan_id": {"type": "integer", "description": "Plan ID to force-complete"}
                },
                "required": ["plan_id"]
            }),
            min_ring: "trusted".into(),
            path_params: vec!["plan_id".into()],
        },
    ]
}
