//! What the AppImage runtime and its AppRun put in the app's environment, and
//! the environment what the app starts gets instead.
//!
//! The runtime mounts the image at `$APPDIR` and names it in `$APPIMAGE` (with
//! `$ARGV0` and `$OWD`). Then AppRun — linuxdeploy's script, which sources its
//! GTK plugin's hook (`apprun-hooks/linuxdeploy-plugin-gtk.sh`) and runs
//! AppImageKit's `AppRun.wrapped` — points the launch's programs, libraries and
//! data into the mount. In the AppImage `desktop/build-linux.sh` builds, it:
//!
//! * puts the mount's folders in front of `PATH`, `LD_LIBRARY_PATH`,
//!   `PERLLIB`, `PYTHONPATH`, `QT_PLUGIN_PATH` and `XDG_DATA_DIRS` ([`PREPENDED`]:
//!   the hook adds `/usr/share` after its own there), and an empty entry where
//!   the variable was unset — the current folder, to the loader;
//! * sets, whatever was there before ([`SET`]): `GTK_PATH`, `GTK_DATA_PREFIX`,
//!   `GTK_EXE_PREFIX`, `GTK_IM_MODULE_FILE`, `GDK_PIXBUF_MODULE_FILE`,
//!   `GIO_MODULE_DIR`, `GI_TYPELIB_PATH`, `GSETTINGS_SCHEMA_DIR` and
//!   `PYTHONHOME`, all into the mount; `GST_PLUGIN_SYSTEM_PATH` and
//!   `GST_PLUGIN_SYSTEM_PATH_1_0`, the mount's and then `QT_PLUGIN_PATH`'s old
//!   value (AppRun's slip); `GTK_THEME`, light or dark Adwaita; and
//!   `PYTHONDONTWRITEBYTECODE`.
//!
//! That is the app's own environment, and right for it. What it starts is not
//! the AppImage — the server, the sessions and terminals the server starts
//! (which outlive the app), the next version after an update — and gets
//! [`outside_appimage`] instead: each variable AppRun added to back as it was
//! before the launch, and each one it set gone, as on the desktop (what was
//! there before is lost, and unset is the system's default). Otherwise a
//! `PYTHONHOME` into the mount keeps Python from starting at all, a terminal's
//! GTK programs come up light Adwaita with the app's modules, and a library
//! path into the mount loads whatever is there: once the app has quit, its
//! mount's name in `/tmp` is free for anyone to make again.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// The runtime's own variables.
const RUNTIME: [&str; 4] = ["APPIMAGE", "APPDIR", "ARGV0", "OWD"];

/// What AppRun puts the mount's folders in front of, the value from before
/// the launch after them (none: an empty entry).
pub const PREPENDED: [&str; 6] = ["PATH", "LD_LIBRARY_PATH", "PERLLIB", "PYTHONPATH", "QT_PLUGIN_PATH", "XDG_DATA_DIRS"];

/// What AppRun and its GTK hook set, whatever was there before.
pub const SET: [&str; 13] = [
    "GTK_PATH",
    "GTK_DATA_PREFIX",
    "GTK_EXE_PREFIX",
    "GTK_IM_MODULE_FILE",
    "GDK_PIXBUF_MODULE_FILE",
    "GIO_MODULE_DIR",
    "GI_TYPELIB_PATH",
    "GSETTINGS_SCHEMA_DIR",
    "PYTHONHOME",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "GTK_THEME",
    "PYTHONDONTWRITEBYTECODE",
];

/// What the GTK hook puts after its own folder in `XDG_DATA_DIRS`.
const HOOK_DATA_DIR: &[u8] = b"/usr/share";

/// `appdir` (`$APPDIR`) when this launch is the AppImage's: the executable
/// `exe` runs from inside it, the image mounted there or extracted there
/// (`--appimage-extract-and-run`). `None` when another AppImage that started
/// this app left it in the environment: what this app starts then gets that
/// environment as it always has.
pub fn own_appdir(appdir: Option<&Path>, exe: Option<&Path>) -> Option<PathBuf> {
    let dir = appdir.filter(|dir| dir.is_absolute())?.canonicalize().ok()?;
    let exe = exe?.canonicalize().ok()?;
    (exe.starts_with(&dir) && exe != dir).then_some(dir)
}

/// What the app starts gets, when this launch is the AppImage's
/// ([`own_appdir`] of `$APPDIR` and the running executable): this process's
/// environment, [`outside_appimage`]. `None` anywhere else (macOS, the .deb,
/// a development build, an app another AppImage started): what it starts then
/// gets the app's environment, as it always has.
pub fn outside_this_appimage() -> Option<Vec<(OsString, OsString)>> {
    let appdir = std::env::var_os("APPDIR").map(PathBuf::from);
    let exe = std::env::current_exe().ok();
    let appdir = own_appdir(appdir.as_deref(), exe.as_deref())?;
    Some(outside_appimage(std::env::vars_os(), &appdir))
}

/// The environment for what the app starts, from this launch's `vars`: without
/// the runtime's variables and those AppRun set ([`SET`]); those it added to
/// ([`PREPENDED`]) as they were before the launch, unset if they were; and
/// in any other variable, no path into `appdir` (`$APPDIR`), a variable left
/// with none dropped.
pub fn outside_appimage(vars: impl IntoIterator<Item = (OsString, OsString)>, appdir: &Path) -> Vec<(OsString, OsString)> {
    let appdir = appdir.as_os_str().as_bytes();
    let inside = |part: &[u8]| {
        !appdir.is_empty()
            && part
                .windows(appdir.len())
                .enumerate()
                .any(|(at, window)| window == appdir && matches!(part.get(at + appdir.len()), None | Some(b'/')))
    };
    let named = |list: &[&str], name: &[u8]| list.iter().any(|known| known.as_bytes() == name);
    vars.into_iter()
        .filter(|(name, _)| !named(&RUNTIME, name.as_bytes()) && !named(&SET, name.as_bytes()))
        .filter_map(|(name, value)| {
            let parts: Vec<&[u8]> = value.as_bytes().split(|byte| *byte == b':').collect();
            if !parts.iter().any(|part| inside(part)) {
                return Some((name, value));
            }
            let mut before = &parts[..];
            if named(&PREPENDED, name.as_bytes()) {
                // AppRun's: the mount's folders first, then what was there.
                let added = before.iter().take_while(|part| inside(part)).count();
                before = &before[added..];
                if added > 0 && name.as_bytes() == b"XDG_DATA_DIRS" && before.first() == Some(&HOOK_DATA_DIR) {
                    before = &before[1..];
                }
            }
            let kept: Vec<&[u8]> = before.iter().copied().filter(|part| !inside(part)).collect();
            // Only empty entries left (the current folder, to a search path):
            // the variable was unset before the launch.
            kept.iter()
                .any(|part| !part.is_empty())
                .then(|| (name, OsString::from_vec(kept.join(&b':'))))
        })
        .collect()
}

/// The folder what the app starts starts in, given `env` from
/// [`outside_appimage`]: its home, as a launcher starts it, rather than the
/// app's own folder, where AppRun runs it — the mount's `usr`, which goes with
/// the launch and which anything started there would hold, as its folder and
/// its `$PWD`. `/` without a home.
pub fn start_folder(env: &[(OsString, OsString)]) -> PathBuf {
    env.iter()
        .find(|(name, _)| name == "HOME")
        .map(|(_, home)| PathBuf::from(home))
        .filter(|home| home.is_absolute() && home.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNT: &str = "/tmp/.mount_DroppeAb12Cd";

    fn var(name: &str, value: &str) -> (OsString, OsString) {
        (OsString::from(name), OsString::from(value))
    }

    /// AppRun's environment, as the AppImage desktop/build-linux.sh builds
    /// makes it (read from a launch of it), over `before`.
    fn apprun(before: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        let old = |name: &str| before.iter().find(|(known, _)| *known == name).map_or("", |(_, value)| value);
        let m = MOUNT;
        let mut env: Vec<(OsString, OsString)> = before.iter().map(|(name, value)| var(name, value)).collect();
        env.retain(|(name, _)| !PREPENDED.iter().chain(&SET).any(|known| name == known));
        for (name, value) in [
            ("APPDIR", m.to_string()),
            ("APPIMAGE", "/home/me/Apps/Droppen.AppImage".to_string()),
            ("ARGV0", "/home/me/Apps/Droppen.AppImage".to_string()),
            ("OWD", "/home/me".to_string()),
            ("GDK_PIXBUF_MODULE_FILE", format!("{m}//usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache")),
            ("GIO_MODULE_DIR", format!("{m}//usr/lib/gio/modules")),
            ("GI_TYPELIB_PATH", format!("{m}//usr/lib/girepository-1.0")),
            (
                "GSETTINGS_SCHEMA_DIR",
                format!("{m}/usr/share/glib-2.0/schemas/:{m}//usr/share/glib-2.0/schemas"),
            ),
            ("GST_PLUGIN_SYSTEM_PATH", format!("{m}/usr/lib/gstreamer:{}", old("QT_PLUGIN_PATH"))),
            (
                "GST_PLUGIN_SYSTEM_PATH_1_0",
                format!("{m}/usr/lib/gstreamer-1.0:{}", old("QT_PLUGIN_PATH")),
            ),
            ("GTK_DATA_PREFIX", m.to_string()),
            ("GTK_EXE_PREFIX", format!("{m}//usr")),
            ("GTK_IM_MODULE_FILE", format!("{m}//usr/lib/gtk-3.0/3.0.0/immodules.cache")),
            ("GTK_PATH", format!("{m}//usr/lib/gtk-3.0")),
            ("GTK_THEME", "Adwaita:light".to_string()),
            (
                "LD_LIBRARY_PATH",
                format!("{m}/usr/lib/:{m}/usr/lib/x86_64-linux-gnu/:{m}/lib/:{}", old("LD_LIBRARY_PATH")),
            ),
            (
                "PATH",
                format!("{m}/usr/bin/:{m}/usr/sbin/:{m}/usr/games/:{m}/bin/:{m}/sbin/:{}", old("PATH")),
            ),
            ("PERLLIB", format!("{m}/usr/share/perl5/:{m}/usr/lib/perl5/:{}", old("PERLLIB"))),
            ("PYTHONDONTWRITEBYTECODE", "1".to_string()),
            ("PYTHONHOME", format!("{m}/usr/")),
            ("PYTHONPATH", format!("{m}/usr/share/pyshared/:{}", old("PYTHONPATH"))),
            (
                "QT_PLUGIN_PATH",
                format!("{m}/usr/lib/qt5/plugins/:{m}/usr/lib64/qt5/plugins/:{}", old("QT_PLUGIN_PATH")),
            ),
            (
                "XDG_DATA_DIRS",
                format!("{m}/usr/share/:{m}/usr/share:/usr/share:{}", old("XDG_DATA_DIRS")),
            ),
        ] {
            env.push(var(name, &value));
        }
        env
    }

    fn sorted(mut env: Vec<(OsString, OsString)>) -> Vec<(OsString, OsString)> {
        env.sort();
        env
    }

    /// Started from a desktop that sets them, everything AppRun added to is
    /// back as it was, and nothing it set is left.
    #[test]
    fn what_the_app_starts_gets_the_desktops_environment_back() {
        let desktop = [
            ("HOME", "/home/me"),
            ("DISPLAY", ":0"),
            ("PATH", "/home/me/.local/bin:/usr/local/bin:/usr/bin:/bin"),
            ("XDG_DATA_DIRS", "/usr/share/ubuntu:/usr/local/share/:/usr/share/:/var/lib/snapd/desktop"),
            ("LD_LIBRARY_PATH", "/opt/cuda/lib64"),
            ("PYTHONPATH", "/home/me/lib/python"),
            ("PERLLIB", "/home/me/perl5/lib"),
            ("QT_PLUGIN_PATH", "/usr/lib/x86_64-linux-gnu/qt6/plugins"),
            ("GTK_THEME", "Yaru-dark"),
            ("GTK_PATH", "/usr/lib/gtk-extra"),
        ];
        let restored = sorted(outside_appimage(apprun(&desktop), Path::new(MOUNT)));
        let mut expected: Vec<(OsString, OsString)> = desktop.iter().map(|(name, value)| var(name, value)).collect();
        // AppRun had already set these over the desktop's: unset, the system's default.
        expected.retain(|(name, _)| name != "GTK_THEME" && name != "GTK_PATH");
        assert_eq!(restored, sorted(expected));
    }

    /// Started with none of them set, none is left set, and no empty entry
    /// (the current folder, to the loader) is left in a search path.
    #[test]
    fn what_was_unset_is_unset_again() {
        let restored = sorted(outside_appimage(apprun(&[("HOME", "/home/me")]), Path::new(MOUNT)));
        assert_eq!(restored, vec![var("HOME", "/home/me")]);
    }

    /// A desktop whose own XDG_DATA_DIRS begins with /usr/share keeps it: only
    /// the hook's copy goes.
    #[test]
    fn only_the_hooks_own_data_dir_goes() {
        let restored = outside_appimage(apprun(&[("XDG_DATA_DIRS", "/usr/share:/usr/local/share")]), Path::new(MOUNT));
        assert!(restored.contains(&var("XDG_DATA_DIRS", "/usr/share:/usr/local/share")), "{restored:?}");
    }

    /// Anything else naming the mount loses those paths, and only those; a
    /// mount whose name starts the same is another one.
    #[test]
    fn any_other_path_into_the_mount_goes() {
        let restored = sorted(outside_appimage(
            [
                var("SOME_PLUGIN_PATH", &format!("{MOUNT}/usr/lib/plugins:/usr/lib/plugins")),
                var("ONLY_THE_MOUNT", &format!("{MOUNT}/usr/share/thing")),
                var("ANOTHER_MOUNT", &format!("{MOUNT}X/usr/lib::/usr/lib/other")),
                var("GDK_BACKEND", "x11"),
            ],
            Path::new(MOUNT),
        ));
        assert_eq!(
            restored,
            sorted(vec![
                var("SOME_PLUGIN_PATH", "/usr/lib/plugins"),
                var("ANOTHER_MOUNT", &format!("{MOUNT}X/usr/lib::/usr/lib/other")),
                var("GDK_BACKEND", "x11"),
            ])
        );
    }

    /// A test binary runs from no AppImage: what it would start gets its
    /// environment as it is.
    #[test]
    fn outside_an_appimage_there_is_nothing_to_restore() {
        if std::env::var_os("APPDIR").is_none() {
            assert_eq!(outside_this_appimage(), None);
        }
    }

    /// What the app starts starts at home; at the root without a home that is
    /// a folder.
    #[test]
    fn what_the_app_starts_starts_at_home() {
        let home = tempfile::tempdir().expect("tempdir");
        let env = [var("PATH", "/usr/bin:/bin"), (OsString::from("HOME"), home.path().as_os_str().to_owned())];
        assert_eq!(start_folder(&env), home.path());
        let gone = home.path().join("gone");
        for env in [
            vec![],
            vec![var("HOME", "home/me")],
            vec![(OsString::from("HOME"), gone.into_os_string())],
        ] {
            assert_eq!(start_folder(&env), Path::new("/"), "{env:?}");
        }
    }

    #[test]
    fn the_launch_is_the_appimages_only_when_it_runs_from_appdir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let appdir = dir.path().join("squashfs-root");
        std::fs::create_dir_all(appdir.join("usr/bin")).expect("appdir");
        let exe = appdir.join("usr/bin/Droppen");
        std::fs::write(&exe, b"").expect("exe");
        let elsewhere = dir.path().join("Droppen");
        std::fs::write(&elsewhere, b"").expect("elsewhere");
        let canonical = appdir.canonicalize().expect("canonical");
        assert_eq!(own_appdir(Some(&appdir), Some(&exe)), Some(canonical));
        // Another AppImage's, left in the environment of what it started.
        assert_eq!(own_appdir(Some(&appdir), Some(&elsewhere)), None);
        assert_eq!(own_appdir(None, Some(&exe)), None);
        assert_eq!(own_appdir(Some(Path::new("squashfs-root")), Some(&exe)), None, "relative");
        assert_eq!(own_appdir(Some(&appdir), Some(&appdir)), None);
        assert_eq!(own_appdir(Some(&dir.path().join("gone")), Some(&exe)), None);
    }
}
