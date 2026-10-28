//! Each subsystem's sub-state, made from [`AppState`].
//!
//! A subsystem's routes take only the services they use, as a state of their
//! own, so they never name `AppState` and cannot reach a service they did not
//! declare. These impls are the one place that knows which of `AppState`'s
//! fields each sub-state is made of.

use gotcha::axum::extract::FromRef;

use super::AppState;
use crate::apps::AppsState;
use crate::files::FilesState;
use crate::notify::NotifyState;
use crate::plugins::routes::PluginsState;
use crate::push::PushState;
use crate::selfupdate::SelfUpdateState;
use crate::stats::StatsState;

impl FromRef<AppState> for PluginsState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            plugins: state.plugins.clone(),
        }
    }
}

impl FromRef<AppState> for StatsState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            paths: state.paths.clone(),
        }
    }
}

impl FromRef<AppState> for PushState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            paths: state.paths.clone(),
        }
    }
}

impl FromRef<AppState> for NotifyState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            paths: state.paths.clone(),
            instance: state.instance.clone(),
        }
    }
}

impl FromRef<AppState> for AppsState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            apps: state.apps.clone(),
        }
    }
}

impl FromRef<AppState> for FilesState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            paths: state.paths.clone(),
        }
    }
}

impl FromRef<AppState> for SelfUpdateState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            desktop_app: state.desktop_app,
            lock: state.lock.clone(),
            paths: state.paths.clone(),
            cancel: state.cancel.clone(),
        }
    }
}
