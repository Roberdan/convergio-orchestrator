//! HTTP API routes for convergio-orchestrator.

use axum::Router;

/// Returns the router for this crate's API endpoints.
pub fn routes() -> Router {
    Router::new()
    // .route("/api/orchestrator/health", get(health))
}
