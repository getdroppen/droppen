//! Opening an address or a folder as the desktop does: through the opener
//! plugin, except in the Linux app's own AppImage launch. There the opener's
//! `xdg-open` would hand the browser or file manager it starts the app's own
//! environment, AppRun's, with its paths into the app's mount, and the app's
//! folder in the mount; GIO starts it instead, in the environment from before
//! AppRun and at home ([`crate::linux_browser::open_outside`]). Anywhere else,
//! as before.

use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;

/// Open `url` in the browser.
#[cfg(not(feature = "e2e"))]
pub fn url<R: Runtime>(app: &AppHandle<R>, url: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if let Some(env) = droppen_desktop_core::appimage::outside_this_appimage() {
        return crate::linux_browser::open_outside(url, &env);
    }
    app.opener().open_url(url, None::<&str>).map_err(|error| error.to_string())
}

/// Open the folder `path` in the file manager.
pub fn folder<R: Runtime>(app: &AppHandle<R>, path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if let Some(env) = droppen_desktop_core::appimage::outside_this_appimage() {
        use gio::prelude::FileExt;
        return crate::linux_browser::open_outside(&gio::File::for_path(path).uri(), &env);
    }
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|error| error.to_string())
}
