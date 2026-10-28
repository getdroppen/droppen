//! Each subsystem's sub-state, made from [`AppState`].
//!
//! A subsystem's routes take only the services they use, as a state of their
//! own, so they never name `AppState` and cannot reach a service they did not
//! declare. These impls are the one place that knows which of `AppState`'s
//! fields each sub-state is made of.

use gotcha::axum::extract::FromRef;

use super::AppState;
use crate::plugins::routes::PluginsState;

impl FromRef<AppState> for PluginsState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            plugins: state.plugins.clone(),
        }
    }
}
