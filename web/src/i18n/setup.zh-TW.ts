// The zh-TW table of `setup.ts`, which says what it covers: a file of its
// own, so that it is in the zh-TW chunk alone (`lang-zh-TW.ts`).

export const setupZhTW: Record<string, string> = {
  "setup code": "設定碼",
  "confirm password": "再輸入一次密碼",
  "create account and sign in": "建立帳號並登入",
  "creating account…": "正在建立帳號…",
  "Or create the account on this device, without a code:": "或者不用設定碼，直接在這台裝置上建立帳號：",
  "a setup code is 16 letters and digits, like ABCD-EFGH-JKMN-PQRS": "設定碼由 16 個字母和數字組成，形如 ABCD-EFGH-JKMN-PQRS",
  "a user name is 1-32 characters of a-z, 0-9, '.', '_' or '-'": "使用者名稱為 1–32 個字元，只能用 a-z、0-9、「.」「_」或「-」",
  "a password is at least 12 characters long": "密碼至少要 12 個字元",
  "a password is at most 256 bytes long": "密碼最多 256 位元組",
  "a password may not start or end with whitespace": "密碼不能以空白字元開頭或結尾",
  "a password may not be the user name": "密碼不能與使用者名稱相同",
  "that password is one of the most common ones; pick another": "這個密碼太常見了，換一個",
  "That is not the setup code this device's server printed. `{{command}} setup-code` on this device shows it again.":
    "這不是這台裝置的伺服器印出的設定碼。在這台裝置上執行 `{{command}} setup-code` 可以再看一次。",
  "The first account is made on this device's own page, not through Droppen.": "第一個帳號要在這台裝置自己的頁面上建立，不能經由 Droppen。",
  "This device already has an account.": "這台裝置已經有帳號了。",
  "Local login is switched off on this device (auth.enabled = false), so there is no account to create here.":
    "這台裝置關閉了本地登入（auth.enabled = false），這裡沒有要建立的帳號。",
  "This device's server has no setup code. Restart it to get one, or create the account on this device with `{{command}} user add <name>`.":
    "這台裝置的伺服器沒有設定碼。重新啟動伺服器即可取得一個，或在這台裝置上用 `{{command}} user add <名字>` 建立帳號。",
  "Too many wrong setup codes. Try again in {{seconds}} s.": "錯誤的設定碼太多了，請 {{seconds}} 秒後再試。",
  "Too many wrong setup codes. Wait a little, then try again.": "錯誤的設定碼太多了，請稍候再試。",
  /* settings › security, when local login is switched off */
  "switched off": "已關閉",
  "Local login is switched off on this device (auth.enabled = false), so its page answers anyone who can reach it. To require a login, remove this setting from the server's configuration and restart the server:":
    "這台裝置關閉了本地登入（auth.enabled = false），任何連得上的人都能存取它的頁面。要啟用登入，從伺服器設定中移除這項設定，再重新啟動伺服器：",
  "Then make its first account on this device:": "然後在這台裝置上建立它的第一個帳號：",
  "Could not ask this device's server about its login.": "無法向這台裝置的伺服器查詢登入狀態。",
  /* settings › security, this device's owner while it has no account */
  "no account yet": "還沒有帳號",
  /* the setup page: nobody signed in, no account yet */
  "This device has no account yet.": "這台裝置還沒有帳號。",
  "This is the Droppen app's own server. In the app's menu-bar menu, choose Open in Browser: it opens this page signed in, and the first account is made there.":
    "這是 Droppen App 自己的伺服器。在 App 的選單列選單中選擇「在瀏覽器中打開」:它會以已登入的狀態開啟這個頁面,第一個帳號就在那裡建立。",
  "On this device: open Droppen from the app, or run `{{command}} open` in a terminal — it opens this page signed in.":
    "在這台裝置上：從 App 開啟 Droppen，或在終端機執行 `{{command}} open`，它會以已登入的狀態開啟這個頁面。",
  "Or create the first account with the one-time setup code this device's server keeps (`{{command}} setup-code` on this device shows it)":
    "或者用這台裝置的伺服器保存的一次性設定碼建立第一個帳號（在這台裝置上執行 `{{command}} setup-code` 可以看到）",
  "That link was already used or has expired. Run `{{command}} open` on this device again.":
    "這個連結已經用過或已過期。請在這台裝置上重新執行 `{{command}} open`。",
  "This device has an account now: sign in.": "這台裝置現在已經有帳號了，請登入。",
  "That link works only on the device itself, not through Droppen.": "這個連結只能在這台裝置本身上使用，不能經由 Droppen。",
  /* the first account, made by this device's owner without the code */
  "You opened this page as this device's owner. This device has no account yet: create one to sign in from other devices; from then on every browser signs in, this one too.":
    "你以這台裝置擁有者的身分開啟了這個頁面。這台裝置還沒有帳號：建立一個，就能從其它裝置登入；從那以後每個瀏覽器都要登入，包括這一個。",
  "The setup code is needed now. `{{command}} setup-code` on this device shows it, or run `{{command}} open` there again.":
    "現在需要設定碼。在這台裝置上執行 `{{command}} setup-code` 可以查看，或在那裡重新執行 `{{command}} open`。",
  "That name belongs to the Droppen app's own account on this device. Pick another.": "這個名稱已被這台裝置上 Droppen App 自己的帳號占用，請換一個。",
};
