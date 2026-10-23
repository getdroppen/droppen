//! The app's own words — the menu bar item and the menus — in the four
//! languages the web app ships, chosen from the system's preferred languages
//! as the web app chooses for `auto` (`web/src/i18n/index.ts`): the first of
//! them, in the system's order, that the app speaks; English when none is.
//!
//! The rule of the web app applies here too (`web/test/ui-wording.test.ts`):
//! a device is a 设备 / 裝置 / デバイス / "device", never a machine or a
//! computer.

/// A language the app speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// 简体中文.
    ZhCn,
    /// 繁體中文.
    ZhTw,
    /// 日本語.
    Ja,
    /// English.
    En,
}

impl Lang {
    /// The language of a BCP 47 tag (`zh-Hans-CN`, `zh-TW`, `ja-JP`, `en-US`),
    /// if the app speaks it. Chinese in Traditional script (`Hant`, else
    /// Taiwan, Hong Kong or Macao) is 繁體中文, any other Chinese 简体中文; the
    /// script, where the tag names one, decides over the region.
    pub fn from_tag(tag: &str) -> Option<Self> {
        let tag = tag.trim().replace('_', "-").to_ascii_lowercase();
        let mut parts = tag.split('-');
        match parts.next() {
            Some("en") => Some(Self::En),
            Some("ja") => Some(Self::Ja),
            Some("zh") => {
                let rest: Vec<&str> = parts.collect();
                if rest.contains(&"hant") {
                    Some(Self::ZhTw)
                } else if rest.contains(&"hans") {
                    Some(Self::ZhCn)
                } else if rest.iter().any(|part| matches!(*part, "tw" | "hk" | "mo")) {
                    Some(Self::ZhTw)
                } else {
                    Some(Self::ZhCn)
                }
            }
            _ => None,
        }
    }

    /// The first of `tags` the app speaks; English when none is.
    pub fn pick<S: AsRef<str>>(tags: impl IntoIterator<Item = S>) -> Self {
        tags.into_iter().find_map(|tag| Self::from_tag(tag.as_ref())).unwrap_or(Self::En)
    }

    /// The system's preferred languages, in order (macOS: Language & Region).
    pub fn system_tags() -> Vec<String> {
        sys_locale::get_locales().collect()
    }

    /// The first of the system's preferred languages the app speaks.
    pub fn system() -> Self {
        Self::pick(Self::system_tags())
    }

    /// `tags` as `init.js` takes them (`__DROPPEN_LANGUAGES__`, inside a
    /// string literal): comma-separated, and only tags of letters, digits,
    /// `-` and `_`.
    pub fn script_list<S: AsRef<str>>(tags: impl IntoIterator<Item = S>) -> String {
        let tags: Vec<String> = tags
            .into_iter()
            .map(|tag| tag.as_ref().to_string())
            .filter(|tag| !tag.is_empty() && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .collect();
        tags.join(",")
    }

    /// The words.
    pub fn words(self) -> &'static Words {
        match self {
            Self::ZhCn => &ZH_CN,
            Self::ZhTw => &ZH_TW,
            Self::Ja => &JA,
            Self::En => &EN,
        }
    }
}

/// Every string the app shows.
#[allow(missing_docs)]
pub struct Words {
    pub open: &'static str,
    pub open_in_browser: &'static str,
    pub start_at_login: &'static str,
    pub allow_at_login: &'static str,
    pub login_item_failed: &'static str,
    pub quit: &'static str,
    pub starting: &'static str,
    pub running: &'static str,
    pub restarting: &'static str,
    pub failed: &'static str,
    pub about: &'static str,
    pub services: &'static str,
    pub hide: &'static str,
    pub hide_others: &'static str,
    pub show_all: &'static str,
    pub edit: &'static str,
    pub undo: &'static str,
    pub redo: &'static str,
    pub cut: &'static str,
    pub copy: &'static str,
    pub paste: &'static str,
    pub select_all: &'static str,
    pub view: &'static str,
    pub reload: &'static str,
    pub fullscreen: &'static str,
    pub window: &'static str,
    pub minimize: &'static str,
    pub zoom: &'static str,
    pub close_window: &'static str,
}

const ZH_CN: Words = Words {
    open: "打开 Droppen",
    open_in_browser: "在浏览器中打开",
    start_at_login: "登录时启动",
    allow_at_login: "到“登录项”中允许 Droppen…",
    login_item_failed: "没能开启 — 打开“登录项”…",
    quit: "退出 Droppen",
    starting: "正在启动…",
    running: "在这台设备上运行中",
    restarting: "正在重新启动…",
    failed: "没能启动",
    about: "关于 Droppen",
    services: "服务",
    hide: "隐藏 Droppen",
    hide_others: "隐藏其他",
    show_all: "全部显示",
    edit: "编辑",
    undo: "撤销",
    redo: "重做",
    cut: "剪切",
    copy: "拷贝",
    paste: "粘贴",
    select_all: "全选",
    view: "显示",
    reload: "重新载入",
    fullscreen: "进入全屏幕",
    window: "窗口",
    minimize: "最小化",
    zoom: "缩放",
    close_window: "关闭窗口",
};

const ZH_TW: Words = Words {
    open: "打開 Droppen",
    open_in_browser: "在瀏覽器中打開",
    start_at_login: "登入時啟動",
    allow_at_login: "到「登入項目」中允許 Droppen…",
    login_item_failed: "沒能開啟 — 打開「登入項目」…",
    quit: "結束 Droppen",
    starting: "正在啟動…",
    running: "在這台裝置上執行中",
    restarting: "正在重新啟動…",
    failed: "無法啟動",
    about: "關於 Droppen",
    services: "服務",
    hide: "隱藏 Droppen",
    hide_others: "隱藏其他",
    show_all: "顯示全部",
    edit: "編輯",
    undo: "還原",
    redo: "重做",
    cut: "剪下",
    copy: "拷貝",
    paste: "貼上",
    select_all: "全選",
    view: "顯示方式",
    reload: "重新載入",
    fullscreen: "進入全螢幕",
    window: "視窗",
    minimize: "縮到最小",
    zoom: "縮放",
    close_window: "關閉視窗",
};

const JA: Words = Words {
    open: "Droppen を開く",
    open_in_browser: "ブラウザで開く",
    start_at_login: "ログイン時に起動",
    allow_at_login: "「ログイン項目」で Droppen を許可…",
    login_item_failed: "オンにできませんでした — 「ログイン項目」を開く…",
    quit: "Droppen を終了",
    starting: "起動中…",
    running: "このデバイスで実行中",
    restarting: "再起動中…",
    failed: "起動できませんでした",
    about: "Droppen について",
    services: "サービス",
    hide: "Droppen を隠す",
    hide_others: "ほかを隠す",
    show_all: "すべてを表示",
    edit: "編集",
    undo: "取り消す",
    redo: "やり直す",
    cut: "カット",
    copy: "コピー",
    paste: "ペースト",
    select_all: "すべてを選択",
    view: "表示",
    reload: "再読み込み",
    fullscreen: "フルスクリーンにする",
    window: "ウインドウ",
    minimize: "しまう",
    zoom: "拡大／縮小",
    close_window: "ウインドウを閉じる",
};

const EN: Words = Words {
    open: "Open Droppen",
    open_in_browser: "Open in Browser",
    start_at_login: "Start at Login",
    allow_at_login: "Allow Droppen in Login Items…",
    login_item_failed: "Could Not Turn On — Open Login Items…",
    quit: "Quit Droppen",
    starting: "Starting…",
    running: "Running on this device",
    restarting: "Restarting…",
    failed: "Could not start",
    about: "About Droppen",
    services: "Services",
    hide: "Hide Droppen",
    hide_others: "Hide Others",
    show_all: "Show All",
    edit: "Edit",
    undo: "Undo",
    redo: "Redo",
    cut: "Cut",
    copy: "Copy",
    paste: "Paste",
    select_all: "Select All",
    view: "View",
    reload: "Reload",
    fullscreen: "Enter Full Screen",
    window: "Window",
    minimize: "Minimize",
    zoom: "Zoom",
    close_window: "Close Window",
};

#[cfg(test)]
mod tests {
    use super::*;

    fn all(words: &Words) -> Vec<&'static str> {
        vec![
            words.open,
            words.open_in_browser,
            words.start_at_login,
            words.allow_at_login,
            words.login_item_failed,
            words.quit,
            words.starting,
            words.running,
            words.restarting,
            words.failed,
            words.about,
            words.services,
            words.hide,
            words.hide_others,
            words.show_all,
            words.edit,
            words.undo,
            words.redo,
            words.cut,
            words.copy,
            words.paste,
            words.select_all,
            words.view,
            words.reload,
            words.fullscreen,
            words.window,
            words.minimize,
            words.zoom,
            words.close_window,
        ]
    }

    #[test]
    fn languages_are_picked_from_the_tag() {
        for tag in ["zh", "zh-CN", "zh-SG", "zh-Hans", "zh-Hans-CN", "zh-Hans-HK", "zh_CN"] {
            assert_eq!(Lang::from_tag(tag), Some(Lang::ZhCn), "{tag}");
        }
        for tag in ["zh-Hant", "zh-TW", "zh-HK", "zh-MO", "zh-Hant-TW", "zh-Hant-CN", "ZH_hk"] {
            assert_eq!(Lang::from_tag(tag), Some(Lang::ZhTw), "{tag}");
        }
        assert_eq!(Lang::from_tag("ja-JP"), Some(Lang::Ja));
        assert_eq!(Lang::from_tag("ja"), Some(Lang::Ja));
        assert_eq!(Lang::from_tag("en-GB"), Some(Lang::En));
        assert_eq!(Lang::from_tag("en-JP"), Some(Lang::En));
        for tag in ["fr-FR", "de", "ko-KR", "jav", "zhx", ""] {
            assert_eq!(Lang::from_tag(tag), None, "{tag}");
        }
    }

    #[test]
    fn the_first_language_the_app_speaks_wins_else_english() {
        assert_eq!(Lang::pick(["fr-FR", "ja-JP", "zh-Hans-CN"]), Lang::Ja);
        assert_eq!(Lang::pick(["de-DE", "zh-Hant-TW", "en-US"]), Lang::ZhTw);
        assert_eq!(Lang::pick(["en-JP", "ja-JP"]), Lang::En);
        assert_eq!(Lang::pick(["fr-FR", "de-DE"]), Lang::En);
        assert_eq!(Lang::pick(Vec::<String>::new()), Lang::En);
    }

    #[test]
    fn the_script_gets_only_tags() {
        assert_eq!(Lang::script_list(["fr-FR", "ja-JP", "zh-Hans-CN"]), "fr-FR,ja-JP,zh-Hans-CN");
        assert_eq!(Lang::script_list(["en_US", "", "x\"y", "a,b", "de</script>", "ja"]), "en_US,ja");
        assert_eq!(Lang::script_list(Vec::<String>::new()), "");
    }

    /// The same words `web/test/ui-wording.test.ts` keeps off the screen.
    #[test]
    fn a_device_is_never_called_a_machine_or_a_computer() {
        for lang in [Lang::ZhCn, Lang::ZhTw, Lang::Ja, Lang::En] {
            for word in all(lang.words()) {
                let lower = word.to_lowercase();
                for banned in ["机器", "電腦", "电脑", "機器", "マシン", "コンピュータ", "端末", "machine", "computer"] {
                    assert!(!lower.contains(banned), "{lang:?}: {word:?} says {banned}");
                }
                assert!(!word.is_empty());
            }
        }
    }
}
