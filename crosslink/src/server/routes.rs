use axum::{
    routing::{get, post},
    Router,
};

use crate::server::{
    handlers::{
        agents::{get_agent, get_agent_status, list_agents, list_locks, list_stale_locks},
        health::health,
        milestones::{
            assign_milestone, close_milestone, create_milestone, get_milestone, list_milestones,
        },
        sessions::{end_session, get_current_session, start_session, work_on_issue},
    },
    state::AppState,
    ws::ws_handler,
};

/// Build the full axum router with all API routes and static file serving.
pub fn build_router(state: AppState, dashboard_dir: Option<std::path::PathBuf>) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        // Agent monitoring
        .route("/agents", get(list_agents))
        .route("/agents/{id}", get(get_agent))
        .route("/agents/{id}/status", get(get_agent_status))
        // Locks
        .route("/locks", get(list_locks))
        .route("/locks/stale", get(list_stale_locks))
        // Sessions
        .route("/sessions/current", get(get_current_session))
        .route("/sessions/start", post(start_session))
        .route("/sessions/end", post(end_session))
        .route("/sessions/work/{id}", post(work_on_issue))
        // Milestones
        .route("/milestones", get(list_milestones).post(create_milestone))
        .route("/milestones/{id}", get(get_milestone))
        .route("/milestones/{id}/assign", post(assign_milestone))
        .route("/milestones/{id}/close", post(close_milestone));

    let mut app = Router::new()
        .nest("/api/v1", api)
        .route("/ws", get(ws_handler))
        .with_state(state);

    // Serve static dashboard files if a directory was provided.
    if let Some(dir) = dashboard_dir {
        use tower_http::services::ServeDir;
        app = app.fallback_service(ServeDir::new(dir));
    }

    app
}
