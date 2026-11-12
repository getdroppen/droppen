//! Signing this device in to Droppen from the app's window
//! (`docs/local-aggregator.md` §4.9; the rules are
//! [`droppen_desktop_core::sign_in`]): the web app opens the sign-in attempt's
//! droppen.app address as a new window; [`opened`] hands it to the default
//! browser whole and follows the device's standing at the app's own server,
//! bringing the window to the front once the device has joined the account.
//! One watch at a time; another sign-in only makes it last longer.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use droppen_desktop_core::sign_in::{self, SignInWatch, Step};
use tauri::{AppHandle, Manager, Runtime, Url};

use crate::shell::Shell;

static WATCH: Mutex<Option<SignInWatch>> = Mutex::new(None);

/// The web app asked for `url`, a sign-in hand-off ([`sign_in::is_sign_in`]):
/// open it in the browser, fragment and all, and watch for the device joining.
pub fn opened<R: Runtime>(app: &AppHandle<R>, url: &Url) {
    let port = app.state::<Arc<Shell>>().port();
    let url = sign_in::browser_url(url, port);
    // The address carries the attempt's state: the host only, in the log.
    let host = url.host_str().unwrap_or_default().to_string();
    #[cfg(feature = "e2e")]
    log::info!(
        "would open the Droppen sign-in at {}://{host} in the browser (e2e build: not opened)",
        url.scheme()
    );
    #[cfg(not(feature = "e2e"))]
    {
        log::info!("opening the Droppen sign-in at {}://{host} in the browser", url.scheme());
        // Not the error: the opener's quotes the address it could not open
        // (on macOS, `open`'s command line), state and all.
        if crate::opening::url(app, url.as_str()).is_err() {
            log::warn!("cannot open the browser for the Droppen sign-in at {}://{host}", url.scheme());
        }
    }
    watch(app);
}

/// Follow the device's standing until it joins (then show the window) or the
/// time is up.
fn watch<R: Runtime>(app: &AppHandle<R>) {
    {
        let mut slot = WATCH.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(running) = slot.as_mut() {
            running.extend(Instant::now());
            return;
        }
        *slot = Some(SignInWatch::new(Instant::now()));
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("droppen-sign-in".into()).spawn(move || loop {
        std::thread::sleep(sign_in::POLL);
        let shell = app.state::<Arc<Shell>>();
        let state = match (shell.port(), shell.login_token()) {
            (Some(port), Some(token)) => sign_in::cloud_state(port, &token, Duration::from_secs(5)),
            _ => None,
        };
        let step = {
            let mut slot = WATCH.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let step = slot.as_mut().map_or(Step::Stop, |w| w.observe(state.as_deref(), Instant::now()));
            if step != Step::Wait {
                *slot = None;
            }
            step
        };
        match step {
            Step::Wait => continue,
            Step::Focus => {
                log::info!("this device joined its Droppen account: showing the window");
                let shower = app.clone();
                let _ = app.run_on_main_thread(move || crate::window::show(&shower));
                return;
            }
            Step::Stop => {
                log::info!("stopped watching the Droppen sign-in");
                return;
            }
        }
    });
    if let Err(error) = spawned {
        log::warn!("cannot watch the Droppen sign-in: {:?}", error.kind());
        *WATCH.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

#[cfg(test)]
mod tests {
    /// Every `log::…!(…)` call in `source`, as written.
    fn log_calls(source: &str) -> Vec<&str> {
        let mut calls = Vec::new();
        let mut rest = source;
        while let Some(at) = rest.find("log::") {
            let call = &rest[at..];
            let end = call.find(");").map_or(call.len(), |end| end + 2);
            calls.push(&call[..end]);
            rest = &call[end..];
        }
        calls
    }

    /// The sign-in address carries the attempt's state, and an error from the
    /// opener quotes the address it could not open: no log line here takes
    /// either — the address by its scheme and host only, an error by its kind.
    #[test]
    fn the_log_never_gets_the_sign_in_address_or_an_error_that_quotes_it() {
        let source = include_str!("sign_in.rs");
        let code = source.split("#[cfg(test)]").next().expect("the module's code");
        let calls = log_calls(code);
        assert!(calls.len() >= 5, "{calls:#?}");
        for call in calls {
            let rest = call.replace("url.scheme()", "").replace("error.kind()", "");
            assert!(!rest.contains("url"), "the address in the log: {call}");
            assert!(!rest.contains("error"), "an error that can quote the address in the log: {call}");
        }
    }
}
