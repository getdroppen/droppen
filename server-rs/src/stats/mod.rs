//! Usage statistics over the local session files.
//!
//! This module is the routing layer for the `/api/stats/*` surface; the
//! aggregation it serves lives in [`usage`], mirroring the split between
//! the former TypeScript server's `app.ts` and `usage.ts`.

pub mod usage;

use std::sync::Arc;

use gotcha::axum::extract::FromRef;
use gotcha::{api, GotchaRouter, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use droppen_http::extract::ApiQuery;
use droppen_http::openapi_ext::ExtraResponses;
use droppen_instance::paths::Paths;
use usage::UsageStats;

/// Query for `GET /api/stats/usage`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct UsageQuery {
    /// UTC calendar days to cover, ending today; defaults to 30, capped at 365.
    pub days: Option<u32>,
}

/// What the stats routes use of the server's state: where the session files
/// are.
#[derive(Clone)]
pub struct StatsState {
    /// Where sessions are discovered.
    pub paths: Arc<Paths>,
}
droppen_http::substate!(StatsState);

/// Token usage aggregated by day and model over the recent window.
#[api(id = "usage_stats", group = "stats", summary = "Token usage")]
pub async fn usage_stats(State(state): State<StatsState>, ApiQuery(query): ApiQuery<UsageQuery>) -> Json<UsageStats> {
    Json(usage::collect_usage(state.paths.as_ref(), query.days).await)
}

impl StatsState {
    /// Register this subsystem's routes.
    pub fn routes<S>(router: GotchaRouter<S>) -> GotchaRouter<S>
    where
        S: Clone + Send + Sync + 'static,
        StatsState: FromRef<S>,
    {
        router.get("/api/stats/usage", usage_stats)
    }
}

/// Everything gotcha cannot express about those routes.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe(
        "GET",
        "/api/stats/usage",
        200,
        "Token usage aggregated by day and model over the recent window",
    );
}
