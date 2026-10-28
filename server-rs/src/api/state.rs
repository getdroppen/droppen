//! Each subsystem's sub-state, made from [`AppState`].
//!
//! A subsystem's routes take only the services they use, as a state of their
//! own, so they never name `AppState` and cannot reach a service they did not
//! declare. These impls are the one place that knows which of `AppState`'s
//! fields each sub-state is made of.

use gotcha::axum::extract::FromRef;

use super::AppState;
use crate::http::app_window::AppWindow;
use crate::models::ModelsState;
use crate::apps::AppsState;
use crate::attention::AttentionState;
use crate::files::FilesState;
use crate::notify::NotifyState;
use crate::plugins::routes::PluginsState;
use crate::prefs::PrefsState;
use crate::push::PushState;
use crate::schedule::ScheduleState;
use crate::screens::ScreensState;
use crate::selfupdate::SelfUpdateState;
use crate::setup::SetupState;
use crate::stats::StatsState;

impl AppState {
    /// The desktop app's window as this server knows it: the port cell
    /// [`crate::setup::Setup::set_listen_port`] fills, and `desktop_app`, read
    /// now.
    pub fn window(&self) -> AppWindow {
        AppWindow::new(self.setup.listen_port_cell(), self.desktop_app)
    }
}

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

impl FromRef<AppState> for PrefsState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            paths: state.paths.clone(),
            refresh: state.refresh.clone(),
        }
    }
}

impl FromRef<AppState> for ScreensState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            paths: state.paths.clone(),
            refresh: state.refresh.clone(),
        }
    }
}

impl FromRef<AppState> for AttentionState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            paths: state.paths.clone(),
            refresh: state.refresh.clone(),
        }
    }
}

impl FromRef<AppState> for ScheduleState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            paths: state.paths.clone(),
            drive: state.drive.clone(),
            global: state.global.clone(),
            cancel: state.cancel.clone(),
        }
    }
}

impl FromRef<AppState> for SetupState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            setup: state.setup.clone(),
            terminals: state.terminals.clone(),
            accounts: state.drive.accounts.clone(),
            paths: state.drive.paths.clone(),
            desktop_app: state.desktop_app,
        }
    }
}

impl FromRef<AppState> for ModelsState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            paths: state.paths.clone(),
            models: state.drive.models.clone(),
            injections: state.drive.injections.clone(),
            accounts: state.drive.accounts.clone(),
        }
    }
}
