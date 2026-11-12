//! The update's words — the menu item that says where the update is, and the
//! release-notes item — in the app's four languages (`crate::i18n`).
//! Templates carry `{version}`, `{current}` and `{percent}`, filled by [`fill`].
//! A failure is one short, honest line: only a network failure says
//! "network".

use droppen_desktop_core::update::{Menu, Problem};

use crate::i18n::Lang;

/// Every string the update shows.
#[allow(missing_docs)]
pub struct Words {
    pub check: &'static str,
    pub checking: &'static str,
    /// `{percent}`
    pub downloading_percent: &'static str,
    pub downloading: &'static str,
    /// `{version}`
    pub restart_to_update: &'static str,
    pub installing: &'static str,
    pub restarting: &'static str,
    /// `{current}`
    pub up_to_date: &'static str,
    pub move_to_applications: &'static str,
    /// Linux's: an AppImage in a folder this user cannot write.
    pub move_to_writable_folder: &'static str,
    /// Linux's: the .deb.
    pub install_newer_package: &'static str,
    pub release_notes: &'static str,
    pub offline: &'static str,
    pub service: &'static str,
    pub signature: &'static str,
    pub location: &'static str,
    /// Linux's [`Problem::Location`]: there are no Applications to move to.
    pub location_writable_folder: &'static str,
    pub install: &'static str,
    pub other: &'static str,
}

impl Words {
    /// The words of `lang`.
    pub fn of(lang: Lang) -> &'static Self {
        match lang {
            Lang::ZhCn => &ZH_CN,
            Lang::ZhTw => &ZH_TW,
            Lang::Ja => &JA,
            Lang::En => &EN,
        }
    }

    /// What the update menu item says.
    pub fn menu(&self, menu: &Menu) -> String {
        match menu {
            Menu::Check | Menu::Unavailable => self.check.to_string(),
            Menu::Checking => self.checking.to_string(),
            Menu::Downloading(Some(percent)) => fill(self.downloading_percent, &[("percent", &percent.to_string())]),
            Menu::Downloading(None) => self.downloading.to_string(),
            Menu::RestartToUpdate(version) => fill(self.restart_to_update, &[("version", version)]),
            Menu::Installing => self.installing.to_string(),
            Menu::Restarting => self.restarting.to_string(),
            Menu::UpToDate(current) => fill(self.up_to_date, &[("current", current)]),
            Menu::MoveToApplications => self.move_to_applications.to_string(),
            Menu::MoveToWritableFolder => self.move_to_writable_folder.to_string(),
            Menu::InstallNewerPackage => self.install_newer_package.to_string(),
            Menu::Failed(problem) => self.problem(*problem).to_string(),
        }
    }

    /// The short line for a failure.
    pub fn problem(&self, problem: Problem) -> &'static str {
        match problem {
            Problem::Offline => self.offline,
            Problem::Service => self.service,
            Problem::Signature => self.signature,
            Problem::Location if cfg!(target_os = "macos") => self.location,
            Problem::Location => self.location_writable_folder,
            Problem::Install => self.install,
            Problem::Other => self.other,
        }
    }
}

/// `template` with each `{name}` replaced by its value.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values
        .iter()
        .fold(template.to_string(), |text, (name, value)| text.replace(&format!("{{{name}}}"), value))
}

const ZH_CN: Words = Words {
    check: "检查更新…",
    checking: "正在检查更新…",
    downloading_percent: "正在下载更新… {percent}%",
    downloading: "正在下载更新…",
    restart_to_update: "重启以更新到 {version}",
    installing: "正在安装更新…",
    restarting: "正在重启…",
    up_to_date: "已是最新版本 ({current})",
    move_to_applications: "移到「应用程序」文件夹后才能更新",
    move_to_writable_folder: "移到你可写入的文件夹后才能更新",
    install_newer_package: "安装新版软件包即可更新",
    release_notes: "更新说明",
    offline: "网络不可用，没能检查更新",
    service: "更新服务暂时不可用，稍后再试",
    signature: "更新未通过签名校验，已丢弃",
    location: "无法在这里更新：请把 Droppen 移到「应用程序」文件夹",
    location_writable_folder: "无法在这里更新：请把 Droppen 移到你可写入的文件夹",
    install: "没能安装更新，Droppen 照常运行",
    other: "没能更新，稍后再试",
};

const ZH_TW: Words = Words {
    check: "檢查更新…",
    checking: "正在檢查更新…",
    downloading_percent: "正在下載更新… {percent}%",
    downloading: "正在下載更新…",
    restart_to_update: "重新啟動以更新到 {version}",
    installing: "正在安裝更新…",
    restarting: "正在重新啟動…",
    up_to_date: "已是最新版本 ({current})",
    move_to_applications: "移到「應用程式」檔案夾後才能更新",
    move_to_writable_folder: "移到你可寫入的檔案夾後才能更新",
    install_newer_package: "安裝新版套件即可更新",
    release_notes: "更新說明",
    offline: "網路無法使用，沒能檢查更新",
    service: "更新服務暫時無法使用，稍後再試",
    signature: "更新未通過簽名驗證，已捨棄",
    location: "無法在這裡更新：請將 Droppen 移到「應用程式」檔案夾",
    location_writable_folder: "無法在這裡更新：請將 Droppen 移到你可寫入的檔案夾",
    install: "沒能安裝更新，Droppen 照常執行",
    other: "沒能更新，稍後再試",
};

const JA: Words = Words {
    check: "アップデートを確認…",
    checking: "アップデートを確認中…",
    downloading_percent: "アップデートをダウンロード中… {percent}%",
    downloading: "アップデートをダウンロード中…",
    restart_to_update: "再起動して {version} にアップデート",
    installing: "アップデートをインストール中…",
    restarting: "再起動中…",
    up_to_date: "最新バージョンです ({current})",
    move_to_applications: "「アプリケーション」フォルダに移動するとアップデートできます",
    move_to_writable_folder: "書き込めるフォルダに移動するとアップデートできます",
    install_newer_package: "新しいパッケージをインストールするとアップデートできます",
    release_notes: "リリースノート",
    offline: "ネットワークに接続できず、アップデートを確認できませんでした",
    service: "アップデートサーバーが一時的に利用できません",
    signature: "アップデートが署名の検証に失敗したため破棄しました",
    location: "ここではアップデートできません：Droppen を「アプリケーション」フォルダに移動してください",
    location_writable_folder: "ここではアップデートできません：Droppen を書き込めるフォルダに移動してください",
    install: "アップデートをインストールできませんでした。Droppen はそのまま動いています",
    other: "アップデートできませんでした。あとでもう一度お試しください",
};

const EN: Words = Words {
    check: "Check for Updates…",
    checking: "Checking for Updates…",
    downloading_percent: "Downloading Update… {percent}%",
    downloading: "Downloading Update…",
    restart_to_update: "Restart to Update to {version}",
    installing: "Installing Update…",
    restarting: "Restarting…",
    up_to_date: "Up to Date ({current})",
    move_to_applications: "Move Droppen to Applications to Update",
    move_to_writable_folder: "Move Droppen to a Folder You Can Write to Update",
    install_newer_package: "Install a Newer Package to Update",
    release_notes: "Release Notes",
    offline: "No Network — Couldn’t Check for Updates",
    service: "Update Service Unavailable — Try Again Later",
    signature: "Update Failed Its Signature Check and Was Discarded",
    location: "Can’t Update Here — Move Droppen to Applications",
    location_writable_folder: "Can’t Update Here — Move Droppen to a Folder You Can Write",
    install: "Couldn’t Install the Update — Droppen Runs as Before",
    other: "Couldn’t Update — Try Again Later",
};

#[cfg(test)]
mod tests {
    use super::*;

    const LANGS: [Lang; 4] = [Lang::ZhCn, Lang::ZhTw, Lang::Ja, Lang::En];
    const PROBLEMS: [Problem; 6] = [
        Problem::Offline,
        Problem::Service,
        Problem::Signature,
        Problem::Location,
        Problem::Install,
        Problem::Other,
    ];

    fn all(words: &Words) -> Vec<&'static str> {
        let mut all = vec![
            words.check,
            words.checking,
            words.downloading_percent,
            words.downloading,
            words.restart_to_update,
            words.installing,
            words.restarting,
            words.up_to_date,
            words.move_to_applications,
            words.move_to_writable_folder,
            words.install_newer_package,
            words.release_notes,
            words.location,
            words.location_writable_folder,
        ];
        all.extend(PROBLEMS.map(|problem| words.problem(problem)));
        all
    }

    fn placeholders(text: &str) -> Vec<&str> {
        let mut found: Vec<&str> = text
            .split('{')
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
            .collect();
        found.sort_unstable();
        found
    }

    #[test]
    fn every_language_fills_the_same_placeholders() {
        let english = all(Words::of(Lang::En));
        for lang in LANGS {
            for (text, reference) in all(Words::of(lang)).into_iter().zip(&english) {
                assert_eq!(placeholders(text), placeholders(reference), "{lang:?}: {text:?}");
                assert!(!text.trim().is_empty(), "{lang:?}");
            }
        }
    }

    /// The rule of `web/test/ui-wording.test.ts` and `crate::i18n`.
    #[test]
    fn a_device_is_never_called_a_machine_or_a_computer() {
        for lang in LANGS {
            for text in all(Words::of(lang)) {
                let lower = text.to_lowercase();
                for banned in [
                    "机器",
                    "電腦",
                    "电脑",
                    "機器",
                    "マシン",
                    "コンピュータ",
                    "端末",
                    "本机",
                    "本機",
                    "machine",
                    "computer",
                ] {
                    assert!(!lower.contains(banned), "{lang:?}: {text:?} says {banned}");
                }
            }
        }
    }

    /// Only a network failure blames the network.
    #[test]
    fn only_the_network_failure_says_network() {
        for lang in LANGS {
            let words = Words::of(lang);
            for problem in PROBLEMS {
                let line = words.problem(problem).to_lowercase();
                let says_network = ["network", "网络", "網路", "ネットワーク"].iter().any(|word| line.contains(word));
                assert_eq!(says_network, problem == Problem::Offline, "{lang:?} {problem:?}: {line}");
            }
        }
    }

    #[test]
    fn the_menu_says_where_the_update_is() {
        let zh = Words::of(Lang::ZhCn);
        assert_eq!(zh.menu(&Menu::Check), "检查更新…");
        assert_eq!(zh.menu(&Menu::Checking), "正在检查更新…");
        assert_eq!(zh.menu(&Menu::UpToDate("0.2.124".into())), "已是最新版本 (0.2.124)");
        assert_eq!(zh.menu(&Menu::Downloading(Some(42))), "正在下载更新… 42%");
        assert_eq!(zh.menu(&Menu::RestartToUpdate("0.2.125".into())), "重启以更新到 0.2.125");
        assert_eq!(zh.menu(&Menu::Failed(Problem::Signature)), "更新未通过签名校验，已丢弃");
        let en = Words::of(Lang::En);
        assert_eq!(en.menu(&Menu::RestartToUpdate("0.2.125".into())), "Restart to Update to 0.2.125");
        assert_eq!(en.menu(&Menu::Downloading(None)), "Downloading Update…");
        assert_eq!(en.menu(&Menu::MoveToApplications), "Move Droppen to Applications to Update");
        assert_eq!(en.menu(&Menu::Unavailable), "Check for Updates…");
        assert_eq!(en.menu(&Menu::MoveToWritableFolder), "Move Droppen to a Folder You Can Write to Update");
        assert_eq!(en.menu(&Menu::InstallNewerPackage), "Install a Newer Package to Update");
        assert_eq!(zh.menu(&Menu::InstallNewerPackage), "安装新版软件包即可更新");
    }

    /// A Linux app has no Applications folder to be moved to.
    #[test]
    fn a_location_failure_names_the_way_out_of_this_platform() {
        let en = Words::of(Lang::En);
        let line = en.menu(&Menu::Failed(Problem::Location));
        if cfg!(target_os = "macos") {
            assert_eq!(line, "Can’t Update Here — Move Droppen to Applications");
        } else {
            assert_eq!(line, "Can’t Update Here — Move Droppen to a Folder You Can Write");
        }
        for lang in LANGS {
            let words = Words::of(lang);
            assert_ne!(words.location, words.location_writable_folder, "{lang:?}");
        }
    }
}
