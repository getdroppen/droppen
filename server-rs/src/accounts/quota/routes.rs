//! `GET /api/stats/quota`.

use gotcha::{api, GotchaContext, GotchaRouter, Json, State};

use super::Quota;
use crate::api::AppState;
use crate::config::AppConfig;
use crate::openapi_ext::ExtraResponses;

/// Subscription quota: Codex rolling-window usage and the Claude plan.
#[api(id = "quota", group = "stats", summary = "Subscription quota")]
pub async fn quota(State(state): State<AppState>) -> Json<Quota> {
    Json(state.drive.quota.collect(&state.drive.accounts).await)
}

/// Register the quota routes.
pub fn routes(router: GotchaRouter<GotchaContext<AppState, AppConfig>>) -> GotchaRouter<GotchaContext<AppState, AppConfig>> {
    router.get("/api/stats/quota", quota)
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
