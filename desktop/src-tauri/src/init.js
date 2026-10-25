// Droppen desktop: runs at the start of every document the app's windows load,
// before any of the page's own scripts. It does three small things for the
// server's web app, which itself knows little about the app
// (droppen-desktop-core/src/web.rs describes the contract):
//
//   1. stores the login the app made, carried in the URL fragment, where the
//      web app reads its token — only when the fragment carries this launch's
//      nonce, so a link made by anyone else cannot plant a login — then removes
//      it, and the marker that made the load a new document, from the address;
//   2. turns the web app's move to its login page into a real navigation, which
//      the app cancels and answers with a fresh login: the page is never shown.
//      Its move to its setup page too: the app's own account is app-internal,
//      so its server is always in its setup state, and a lapsed login sends
//      the web app there instead — which here becomes the login page, coming
//      back to the page being left. And it sends plain links off the app to
//      the browser;
//   3. shows one small notice when macOS runs the app from a temporary copy.
//      Whether Claude Code or Codex is ready is said by the web app's
//      first-run guide (web/src/features/setup), not here;
//   4. carries the app's update status to the web app's update page, and its
//      "check" and "restart" back to the app;
//   5. lists the system's preferred languages, all of them, in
//      navigator.languages: WebKit lists only the first, so a page's "auto"
//      (web/src/i18n/index.ts) would show English to a reader whose first
//      language the app does not speak, while the app's menus take the first
//      one it does (src-tauri/src/i18n.rs). Only on the app's own pages: the
//      start-up page (tauri:), a new window's about:blank, and the server's
//      http://127.0.0.1; any other page keeps WebKit's one language.
//
// __DROPPEN_NONCE__ is replaced with this launch's nonce when the window is made,
// __DROPPEN_BRIDGE__ with the update bridge's own token (part 4),
// __DROPPEN_LANGUAGES__ with the system's languages, comma-separated (part 5).
//
// It runs on any http://127.0.0.1 page, whatever its port: the server's port
// can change while the window lives, and this script is fixed when the window
// is made. The port is the app's to check, and it does before it hands a page
// anything (src-tauri/src/updater.rs, window.rs). Nothing here gives a page
// the nonce or the bridge token; they only decide what this script accepts.
(function () {
  "use strict";

  var server = location.protocol === "http:" && location.hostname === "127.0.0.1";

  // 5. The system's languages, in its order, on the app's own pages only.
  var LANGUAGES = "__DROPPEN_LANGUAGES__".split(",").filter(function (tag) {
    return /^[A-Za-z]{2,8}([_-][A-Za-z0-9]{1,8})*$/.test(tag);
  });
  if (LANGUAGES.length && (server || location.protocol === "tauri:" || location.protocol === "about:")) {
    Object.freeze(LANGUAGES);
    // The list is the system's as the window was made. WebKit's own
    // navigator.language is live: when it no longer names the list's first
    // language (the system's changed since), it goes first. WebKit spells some
    // tags its own way (en-JP as en-US), which then lists one language twice:
    // harmless, as only the first the app speaks counts.
    var shownFor = null;
    var shown = LANGUAGES;
    try {
      Object.defineProperty(navigator, "languages", {
        configurable: true,
        enumerable: true,
        get: function () {
          var live = navigator.language || "";
          if (live !== shownFor) {
            shownFor = live;
            var first = live.toLowerCase();
            shown =
              !live || first === LANGUAGES[0].toLowerCase()
                ? LANGUAGES
                : Object.freeze(
                    [live].concat(
                      LANGUAGES.filter(function (tag) {
                        return tag.toLowerCase() !== first;
                      }),
                    ),
                  );
          }
          return shown;
        },
      });
    } catch (e) {
      // WebKit's one language stays
    }
  }

  if (!server) return;

  var NONCE = "__DROPPEN_NONCE__";
  var TOKEN_KEY = "rh.auth.token"; // web/src/lib/auth.ts, TOKEN_KEY
  var NONCE_MARK = "droppen-desktop-nonce";
  var TOKEN_MARK = "droppen-desktop-token";
  var MOVED_MARK = "droppen-desktop-moved";
  var LOAD_MARK = "droppen-desktop-load";
  var MOVED_PENDING = "droppen.desktop.moved";
  var MOVED_DISMISSED = "droppen.desktop.moved-dismissed";
  var LOGIN_PATH = "/login";
  var SETUP_PATH = "/setup"; // web/src/lib/auth.ts, SETUP_PATH

  function remember(key, on) {
    try {
      if (on) sessionStorage.setItem(key, "1");
      else sessionStorage.removeItem(key);
    } catch (e) {
      // the notices are best effort
    }
  }

  // 1. The login, and the marker that made this load a new document.
  var here = new URL(location.href);
  var cleaned = false;
  if (here.searchParams.has(LOAD_MARK)) {
    here.searchParams.delete(LOAD_MARK);
    cleaned = true;
  }
  var hash = here.hash.replace(/^#/, "");
  if (hash.indexOf(TOKEN_MARK + "=") !== -1) {
    var params = {};
    hash.split("&").forEach(function (pair) {
      var at = pair.indexOf("=");
      if (at > 0) {
        try {
          params[pair.slice(0, at)] = decodeURIComponent(pair.slice(at + 1));
        } catch (e) {
          // a malformed pair is skipped
        }
      }
    });
    if (params[NONCE_MARK] === NONCE && params[TOKEN_MARK]) {
      try {
        localStorage.setItem(TOKEN_KEY, params[TOKEN_MARK]);
      } catch (e) {
        // storage unavailable: the web app will ask for a login, which the app answers
      }
      remember(MOVED_PENDING, params[MOVED_MARK] === "no");
    }
    // Gone from the address either way: a token is never left on show.
    here.hash = "";
    cleaned = true;
  }
  if (cleaned) history.replaceState(history.state, "", here.pathname + here.search + here.hash);

  // 2. The login and setup pages, never shown.
  function signInPage(url) {
    try {
      var target = new URL(url, location.href);
      if (target.origin !== location.origin) return null;
      if (target.pathname === LOGIN_PATH || target.pathname === SETUP_PATH) return target;
    } catch (e) {
      // not a URL: not one of ours
    }
    return null;
  }
  // The web app's own rule (safeNext): one leading "/", and never "//" or
  // "/\", which a URL parser reads as another host; tabs and newlines go first,
  // as a URL parser drops them.
  function safeNext(path) {
    var cleaned = String(path || "").replace(/[\t\n\r]/g, "");
    if (cleaned.charAt(0) !== "/" || cleaned.charAt(1) === "/" || cleaned.charAt(1) === "\\") return "/";
    return cleaned;
  }
  ["pushState", "replaceState"].forEach(function (name) {
    var original = history[name];
    history[name] = function (state, title, url) {
      var target = url != null ? signInPage(url) : null;
      if (target) {
        if (target.pathname === SETUP_PATH) {
          // The setup page says nothing of where it came from: the login page
          // does, with the page this document is on — the one being left.
          var onSignIn = location.pathname === LOGIN_PATH || location.pathname === SETUP_PATH;
          var next = onSignIn ? "/" : safeNext(location.pathname + location.search);
          target = new URL(LOGIN_PATH, location.href);
          if (next !== "/") target.searchParams.set("next", next);
        }
        location.replace(target.href);
        return;
      }
      return original.apply(this, arguments);
    };
  });

  // 2b. A plain link off the app opens in the browser, not in this window: it
  // becomes a new-window request, which the app hands to the browser. (Frames
  // may load the web; only the page itself is kept on the app.)
  document.addEventListener(
    "click",
    function (event) {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      var link = event.target && event.target.closest ? event.target.closest("a[href]") : null;
      if (!link || link.hasAttribute("download")) return;
      var target = (link.getAttribute("target") || "").toLowerCase();
      if (target && target !== "_self" && target !== "_top" && target !== "_parent") return;
      var url;
      try {
        url = new URL(link.href, location.href);
      } catch (e) {
        return;
      }
      if ((url.protocol !== "http:" && url.protocol !== "https:") || url.origin === location.origin) return;
      event.preventDefault();
      window.open(url.href, "_blank", "noopener");
    },
    true,
  );

  // 3. The notices.
  var NOTICES = {
    moved: {
      id: "droppen-desktop-moved-notice",
      pending: MOVED_PENDING,
      dismissed: MOVED_DISMISSED,
      // For this run only: moving the app is the fix, and the next run checks again.
      remember: sessionStorage,
      links: {},
      text: {
        "zh-CN": {
          text: "macOS 正在从一份临时副本运行 Droppen。把 Droppen 移到「应用程序」文件夹，再从那里打开，登录时启动才能正常工作。",
          dismiss: "知道了",
        },
        "zh-TW": {
          text: "macOS 正在從一份暫時副本執行 Droppen。把 Droppen 移到「應用程式」檔案夾，再從那裡打開，登入時啟動才能正常運作。",
          dismiss: "知道了",
        },
        ja: {
          text: "macOS が Droppen を一時的なコピーから実行しています。Droppen を「アプリケーション」フォルダに移動してから開くと、ログイン時の起動が正しく動作します。",
          dismiss: "閉じる",
        },
        en: {
          text: "macOS is running Droppen from a temporary copy. Move Droppen to the Applications folder and open it from there, so Start at Login works.",
          dismiss: "Dismiss",
        },
      },
    },
  };

  function language() {
    var lang = (document.documentElement.lang || navigator.language || "en").toLowerCase();
    if (/^zh/.test(lang)) return /tw|hk|mo|hant/.test(lang) ? "zh-TW" : "zh-CN";
    if (/^ja/.test(lang)) return "ja";
    return "en";
  }

  function show(notice, bottom) {
    try {
      if (sessionStorage.getItem(notice.pending) !== "1" || notice.remember.getItem(notice.dismissed) === "1") return bottom;
    } catch (e) {
      return bottom;
    }
    if (document.getElementById(notice.id) || !document.body) return bottom;
    var t = notice.text[language()];
    var box = document.createElement("div");
    box.id = notice.id;
    box.setAttribute("role", "status");
    box.style.cssText =
      "position:fixed;right:16px;bottom:" + bottom + "px;z-index:2147483647;max-width:360px;box-sizing:border-box;" +
      "padding:12px 14px;background:var(--color-bg,#fff);color:var(--color-fg,#0a0a0a);" +
      "border:1px solid var(--color-border-strong,#a3a3a3);font:12px/1.5 var(--font-sans,-apple-system,system-ui,sans-serif);" +
      "box-shadow:0 1px 0 rgba(0,0,0,.04)";
    var text = document.createElement("p");
    text.style.cssText = "margin:0 0 10px";
    text.textContent = t.text;
    box.appendChild(text);
    var row = document.createElement("div");
    row.style.cssText = "display:flex;flex-wrap:wrap;align-items:center;gap:12px";
    Object.keys(notice.links).forEach(function (which) {
      var link = document.createElement("a");
      link.href = notice.links[which];
      link.target = "_blank";
      link.rel = "noreferrer";
      link.textContent = t[which];
      link.style.cssText = "color:inherit;text-decoration:underline;text-underline-offset:3px";
      row.appendChild(link);
    });
    var dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.textContent = t.dismiss;
    dismiss.style.cssText =
      "margin-left:auto;height:24px;padding:0 10px;border:1px solid var(--color-border-strong,#a3a3a3);" +
      "background:transparent;color:inherit;font:inherit;cursor:pointer";
    dismiss.addEventListener("click", function () {
      try {
        notice.remember.setItem(notice.dismissed, "1");
      } catch (e) {
        // dismissed for this page only
      }
      box.remove();
    });
    row.appendChild(dismiss);
    box.appendChild(row);
    document.body.appendChild(box);
    return bottom + box.offsetHeight + 8;
  }

  // Late enough for the web app to have set its language on <html>.
  function later() {
    setTimeout(function () {
      show(NOTICES.moved, 16);
    }, 1500);
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", later);
  else later();

  // 4. The update (src-tauri/src/updater.rs; web/src/app/desktop-update.tsx),
  // until the app's control channel to the server replaces this bridge. The
  // app calls update(bridge, status, restart) in the server's pages after
  // every change and every page load; only a call with the bridge's token —
  // the update's own, never the login nonce — is kept. The web app reads
  // updateState() whenever a `droppen:update` event says it changed;
  // checkForUpdates() and restartToUpdate() are navigations the app cancels
  // and answers — the first by checking now, the second by restarting into
  // the new version only if one is downloaded and verified. Both carry the
  // token the app handed over, which asks for nothing but that.
  var BRIDGE = "__DROPPEN_BRIDGE__";
  var updateStatus = null;
  var restartToken = null;
  try {
    Object.defineProperty(window, "__droppenDesktop", {
      value: Object.freeze({
        update: function (bridge, status, restart) {
          if (bridge !== BRIDGE) return;
          updateStatus = status && typeof status === "object" ? JSON.stringify(status) : null;
          restartToken = typeof restart === "string" && restart ? restart : null;
          window.dispatchEvent(new CustomEvent("droppen:update"));
        },
        updateState: function () {
          return updateStatus ? JSON.parse(updateStatus) : null;
        },
        checkForUpdates: function () {
          if (!restartToken) return;
          location.assign("droppen-desktop://update/check?token=" + encodeURIComponent(restartToken));
        },
        restartToUpdate: function () {
          if (!restartToken) return;
          location.assign("droppen-desktop://update/restart?token=" + encodeURIComponent(restartToken));
        },
      }),
    });
  } catch (e) {
    // defined already in this document
  }
})();
