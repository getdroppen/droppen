//! `GET /api/stats/quota`.

use std::sync::Arc;

use gotcha::axum::extract::FromRef;
use gotcha::{api, GotchaRouter, Json, State};

use super::{Quota, QuotaCache};
use crate::accounts::store::AccountsStore;
use droppen_http::openapi_ext::ExtraResponses;

/// What the quota route uses of the server's state.
#[derive(Clone)]
pub struct QuotaState {
    /// The cached readings.
    pub quota: Arc<QuotaCache>,
    /// The stored accounts, whose readings are collected.
    pub accounts: Arc<AccountsStore>,
}
droppen_http::substate!(QuotaState);

/// Subscription quota: Codex rolling-window usage and the Claude plan.
#[api(id = "quota", group = "stats", summary = "Subscription quota")]
pub async fn quota(State(state): State<QuotaState>) -> Json<Quota> {
    Json(state.quota.collect(&state.accounts).await)
}

impl QuotaState {
    /// Register the quota routes.
    pub fn routes<S>(router: GotchaRouter<S>) -> GotchaRouter<S>
    where
        S: Clone + Send + Sync + 'static,
        QuotaState: FromRef<S>,
    {
        router.get("/api/stats/quota", quota)
    }
}

/// The response wordings the golden contract carries.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe(
        "GET",
        "/api/stats/quota",
        200,
        "Subscription quota: Codex rolling-window usage and the Claude plan",
    );
}
