//! Each subsystem's sub-state, made from [`AppState`].
//!
//! A subsystem's routes take only the services they use, as a state of their
//! own, so they never name `AppState` and cannot reach a service they did not
//! declare. These impls are the one place that knows which of `AppState`'s
//! fields each sub-state is made of.

use std::sync::Arc;

use gotcha::axum::extract::FromRef;

use super::AppState;
use crate::accounts::quota::QuotaState;
use crate::accounts::setup_token::TokenWizardState;
use crate::accounts::usage_login::UsageLoginState;
use crate::accounts::AccountsState;
use crate::acp_agent::AcpAgentState;
use droppen_http::app_window::AppWindow;
use crate::models::ModelsState;
use crate::apps::AppsState;
use crate::attention::AttentionState;
use crate::auth::AuthState;
use crate::chats::ChatsState;
use crate::cloud::CloudState;
use crate::drive::DriveState;
use crate::feed::global::EventsState;
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
use crate::terminals::TerminalsState;

impl AppState {
    /// The desktop app's window as this server knows it: the port cell
    /// [`crate::setup::Setup::set_listen_port`] fills, and `desktop_app`, read
    /// now.
    pub fn window(&self) -> AppWindow {
        self.setup.window(self.desktop_app)
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
            transcripts: state.drive.transcripts.clone(),
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
            transcripts: state.drive.transcripts.clone(),
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

impl FromRef<AppState> for AccountsState {
    fn from_ref(state: &AppState) -> Self {
        let injections = state.drive.injections.clone();
        Self {
            accounts: state.drive.accounts.clone(),
            quota: state.drive.quota.clone(),
            paths: state.drive.paths.clone(),
            models: ModelsState::from_ref(state),
            recheck_holds: Arc::new(move || injections.recheck_holds()),
        }
    }
}

impl FromRef<AppState> for QuotaState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            quota: state.drive.quota.clone(),
            accounts: state.drive.accounts.clone(),
        }
    }
}

impl FromRef<AppState> for TokenWizardState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            drive: state.drive.clone(),
            terminals: state.terminals.clone(),
            app_window: state.window(),
        }
    }
}

impl FromRef<AppState> for UsageLoginState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            drive: state.drive.clone(),
            terminals: state.terminals.clone(),
        }
    }
}

impl FromRef<AppState> for DriveState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            drive: state.drive.clone(),
            paths: state.paths.clone(),
            global: state.global.clone(),
            refresh: state.refresh.clone(),
        }
    }
}

impl FromRef<AppState> for ChatsState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            chats: state.chats.clone(),
            paths: state.paths.clone(),
            drive: state.drive.clone(),
            global: state.global.clone(),
        }
    }
}

impl FromRef<AppState> for AcpAgentState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            paths: state.paths.clone(),
            injections: state.drive.injections.clone(),
        }
    }
}

impl FromRef<AppState> for AuthState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            auth: state.drive.auth.clone(),
            paths: state.paths.clone(),
            desktop_app: state.desktop_app,
            link: state.cloud.clone(),
        }
    }
}

impl FromRef<AppState> for EventsState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            send_limit: state.send_limit,
            cancel: state.cancel.clone(),
            plugins: state.plugins.clone(),
            global: state.global.clone(),
            drive: state.drive.clone(),
            instance: state.instance.clone(),
            started_at: state.started_at.clone(),
            feeds: state.feeds.clone(),
        }
    }
}

impl FromRef<AppState> for TerminalsState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            terminals: state.terminals.clone(),
            auth: state.drive.auth.clone(),
            send_limit: state.send_limit,
            cancel: state.cancel.clone(),
        }
    }
}

impl FromRef<AppState> for CloudState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            cloud: state.cloud.clone(),
            auth: state.drive.auth.clone(),
            desktop_app: state.desktop_app,
            cancel: state.cancel.clone(),
        }
    }
}
