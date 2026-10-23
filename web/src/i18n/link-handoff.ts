/*
 * Signing a device in to Droppen through the system browser (the browser
 * hand-off, docs/local-aggregator.md §4.9): "continue in your browser" on the
 * device's page (`features/link/browser-sign-in.tsx`), droppen.app's `/link`
 * answering through a tab (`features/link/link-popup.tsx`), and the device's
 * `/link/return` (`features/link/link-return.tsx`). Merged into the link
 * tables (`link.ts`); keys are the English source strings, as everywhere.
 */

export const linkHandoffZhCN: Record<string, string> = {
  // The device's page.
  "Continue in your browser": "在浏览器里继续",
  "Sign in to droppen.app, or create an account.": "登录 droppen.app，或者注册一个账户。",
  "Check that the fingerprint there matches this one, then add this device.": "核对那边显示的指纹和这里一致，然后加入这台设备。",
  "Come back here: this device joins your account by itself.": "回到这里，这台设备会自己加入你的账户。",
  "waiting for droppen.app… the link works for another {{left}}": "等待 droppen.app… 链接还能用 {{left}}",
  "waiting for droppen.app…": "等待 droppen.app…",
  "open the browser again": "重新打开浏览器",
  "copy link": "复制链接",
  "The wrong browser opened? Copy the link into the one you use droppen.app in, on this device.": "打开的不是你常用的浏览器？复制链接，粘贴到这台设备上你登录 droppen.app 的那个浏览器里。",
  "Whoever opens this link can add this device to their own Droppen account: never send it to anyone, and open it only on this device.":
    "谁打开这个链接，谁就能把这台设备加进自己的 Droppen 账户：不要发给任何人，只在这台设备上打开。",
  "continue in a browser tab instead": "改在浏览器标签页里继续",
  // droppen.app's /link.
  "handing the code to the device…": "正在把注册码交给设备…",
  "Added {{id}}. Go back to Droppen on that device: it comes online in a moment. You can close this tab.":
    "已加入 {{id}}。回到那台设备上的 Droppen，它马上就会上线。这个标签页可以关掉了。",
  "This sign-in link has expired. Start again from Droppen on the device: Settings → Access → Droppen → Sign in to Droppen.":
    "这个登录链接已过期。请在那台设备的 Droppen 里重新开始：设置 → 访问 → Droppen → 登录 Droppen。",
  "Cancelled: nothing was added. You can close this tab.": "已取消，没有加入任何设备。这个标签页可以关掉了。",
  "Check that Droppen on that device shows the same fingerprint. When you add it, a new tab hands the code to that device, then closes.":
    "请核对那台设备上的 Droppen 显示的是同一串指纹。加入时会打开一个新标签页，把注册码交给那台设备，然后自动关闭。",
  "The browser did not open the tab that hands the code to the device. Open it here:": "浏览器没有打开把注册码交给设备的标签页。请在这里打开：",
  "hand the code to the device": "把注册码交给设备",
  "If the device's tab says it did not take the code, cancel here and start again from Droppen on the device.":
    "如果那台设备的标签页说没有收下注册码，就在这里取消，然后在那台设备的 Droppen 里重新开始。",
  // The device's /link/return.
  "Signing in to Droppen": "登录 Droppen",
  "handing the sign-in to Droppen on this device…": "正在把登录交给这台设备上的 Droppen…",
  "This device has the sign-in.": "这台设备已收到登录。",
  "droppen.app, in the other tab, finishes adding it. You can close this tab and go back to Droppen.":
    "droppen.app 会在另一个标签页里完成加入。这个标签页可以关掉，回到 Droppen 即可。",
  "Nothing to hand over here. To sign this device in, open Droppen on it: Settings → Access → Droppen → Sign in to Droppen.":
    "这里没有要交接的内容。要让这台设备登录，请在它上面打开 Droppen：设置 → 访问 → Droppen → 登录 Droppen。",
  "This sign-in ran out of time. Start again from Droppen on this device.": "这次登录已超时。请在这台设备的 Droppen 里重新开始。",
  "This sign-in is no longer open on this device: it was used, cancelled or started again. If it did not finish, start again from Droppen on this device.":
    "这台设备上已没有这次登录：它已被使用、取消或重新开始。如果没有完成，请在这台设备的 Droppen 里重新开始。",
  "This answer does not belong to the sign-in open on this device. Nothing was added.": "这个答复不属于这台设备上正在进行的登录。没有加入任何设备。",
  "This device is in a Droppen account already.": "这台设备已经在一个 Droppen 账户里了。",
  "This device could not reach droppen.app. Check its connection, then start again from Droppen on this device.":
    "这台设备连不上 droppen.app。请检查网络，然后在这台设备的 Droppen 里重新开始。",
  "Droppen on this device did not answer. Is it still running?": "这台设备上的 Droppen 没有响应。它还在运行吗？",
  "This device could not take the sign-in: {{message}}": "这台设备没能接收登录：{{message}}",
};

export const linkHandoffZhTW: Record<string, string> = {
  "Continue in your browser": "在瀏覽器裡繼續",
  "Sign in to droppen.app, or create an account.": "登入 droppen.app，或註冊一個帳戶。",
  "Check that the fingerprint there matches this one, then add this device.": "核對那邊顯示的指紋和這裡一致，然後加入這台裝置。",
  "Come back here: this device joins your account by itself.": "回到這裡，這台裝置會自己加入你的帳戶。",
  "waiting for droppen.app… the link works for another {{left}}": "等待 droppen.app… 連結還能用 {{left}}",
  "waiting for droppen.app…": "等待 droppen.app…",
  "open the browser again": "重新打開瀏覽器",
  "copy link": "複製連結",
  "The wrong browser opened? Copy the link into the one you use droppen.app in, on this device.": "打開的不是你常用的瀏覽器？複製連結，貼到這台裝置上你登入 droppen.app 的那個瀏覽器裡。",
  "Whoever opens this link can add this device to their own Droppen account: never send it to anyone, and open it only on this device.":
    "誰打開這個連結，誰就能把這台裝置加進自己的 Droppen 帳戶：不要傳給任何人，只在這台裝置上打開。",
  "continue in a browser tab instead": "改在瀏覽器分頁裡繼續",
  "handing the code to the device…": "正在把註冊碼交給裝置…",
  "Added {{id}}. Go back to Droppen on that device: it comes online in a moment. You can close this tab.":
    "已加入 {{id}}。回到那台裝置上的 Droppen，它很快就會上線。這個分頁可以關掉了。",
  "This sign-in link has expired. Start again from Droppen on the device: Settings → Access → Droppen → Sign in to Droppen.":
    "這個登入連結已過期。請在那台裝置的 Droppen 裡重新開始：設定 → 存取 → Droppen → 登入 Droppen。",
  "Cancelled: nothing was added. You can close this tab.": "已取消，沒有加入任何裝置。這個分頁可以關掉了。",
  "Check that Droppen on that device shows the same fingerprint. When you add it, a new tab hands the code to that device, then closes.":
    "請核對那台裝置上的 Droppen 顯示的是同一串指紋。加入時會打開一個新分頁，把註冊碼交給那台裝置，然後自動關閉。",
  "The browser did not open the tab that hands the code to the device. Open it here:": "瀏覽器沒有打開把註冊碼交給裝置的分頁。請在這裡打開：",
  "hand the code to the device": "把註冊碼交給裝置",
  "If the device's tab says it did not take the code, cancel here and start again from Droppen on the device.":
    "如果那台裝置的分頁說沒有收下註冊碼，就在這裡取消，然後在那台裝置的 Droppen 裡重新開始。",
  "Signing in to Droppen": "登入 Droppen",
  "handing the sign-in to Droppen on this device…": "正在把登入交給這台裝置上的 Droppen…",
  "This device has the sign-in.": "這台裝置已收到登入。",
  "droppen.app, in the other tab, finishes adding it. You can close this tab and go back to Droppen.":
    "droppen.app 會在另一個分頁裡完成加入。這個分頁可以關掉，回到 Droppen 即可。",
  "Nothing to hand over here. To sign this device in, open Droppen on it: Settings → Access → Droppen → Sign in to Droppen.":
    "這裡沒有要交接的內容。要讓這台裝置登入，請在它上面打開 Droppen：設定 → 存取 → Droppen → 登入 Droppen。",
  "This sign-in ran out of time. Start again from Droppen on this device.": "這次登入已逾時。請在這台裝置的 Droppen 裡重新開始。",
  "This sign-in is no longer open on this device: it was used, cancelled or started again. If it did not finish, start again from Droppen on this device.":
    "這台裝置上已沒有這次登入：它已被使用、取消或重新開始。如果沒有完成，請在這台裝置的 Droppen 裡重新開始。",
  "This answer does not belong to the sign-in open on this device. Nothing was added.": "這個回覆不屬於這台裝置上正在進行的登入。沒有加入任何裝置。",
  "This device is in a Droppen account already.": "這台裝置已經在一個 Droppen 帳戶裡了。",
  "This device could not reach droppen.app. Check its connection, then start again from Droppen on this device.":
    "這台裝置連不上 droppen.app。請檢查網路，然後在這台裝置的 Droppen 裡重新開始。",
  "Droppen on this device did not answer. Is it still running?": "這台裝置上的 Droppen 沒有回應。它還在執行嗎？",
  "This device could not take the sign-in: {{message}}": "這台裝置沒能接收登入：{{message}}",
};

export const linkHandoffJa: Record<string, string> = {
  "Continue in your browser": "ブラウザで続けてください",
  "Sign in to droppen.app, or create an account.": "droppen.app にサインインするか、アカウントを作成します。",
  "Check that the fingerprint there matches this one, then add this device.": "表示されたフィンガープリントがこれと一致することを確認して、このデバイスを追加します。",
  "Come back here: this device joins your account by itself.": "ここに戻ると、このデバイスが自動でアカウントに加わります。",
  "waiting for droppen.app… the link works for another {{left}}": "droppen.app を待っています… リンクはあと {{left}} 有効です",
  "waiting for droppen.app…": "droppen.app を待っています…",
  "open the browser again": "ブラウザをもう一度開く",
  "copy link": "リンクをコピー",
  "The wrong browser opened? Copy the link into the one you use droppen.app in, on this device.": "違うブラウザが開きましたか？リンクをコピーして、このデバイスで droppen.app を使っているブラウザに貼り付けてください。",
  "Whoever opens this link can add this device to their own Droppen account: never send it to anyone, and open it only on this device.":
    "このリンクを開いた人は、このデバイスを自分の Droppen アカウントに追加できます。誰にも送らず、このデバイスでだけ開いてください。",
  "continue in a browser tab instead": "代わりにブラウザのタブで続ける",
  "handing the code to the device…": "コードをデバイスに渡しています…",
  "Added {{id}}. Go back to Droppen on that device: it comes online in a moment. You can close this tab.":
    "{{id}} を追加しました。そのデバイスの Droppen に戻ってください。まもなくオンラインになります。このタブは閉じてかまいません。",
  "This sign-in link has expired. Start again from Droppen on the device: Settings → Access → Droppen → Sign in to Droppen.":
    "このサインインのリンクは期限切れです。そのデバイスの Droppen でやり直してください：設定 → アクセス → Droppen → Droppen にサインイン。",
  "Cancelled: nothing was added. You can close this tab.": "キャンセルしました。何も追加されていません。このタブは閉じてかまいません。",
  "Check that Droppen on that device shows the same fingerprint. When you add it, a new tab hands the code to that device, then closes.":
    "そのデバイスの Droppen に同じフィンガープリントが表示されていることを確認してください。追加すると新しいタブが開いてコードをそのデバイスに渡し、自動で閉じます。",
  "The browser did not open the tab that hands the code to the device. Open it here:": "コードをデバイスに渡すタブをブラウザが開きませんでした。ここから開いてください：",
  "hand the code to the device": "コードをデバイスに渡す",
  "If the device's tab says it did not take the code, cancel here and start again from Droppen on the device.":
    "デバイス側のタブにコードを受け取れなかったと表示されたら、ここでキャンセルして、そのデバイスの Droppen でやり直してください。",
  "Signing in to Droppen": "Droppen へのサインイン",
  "handing the sign-in to Droppen on this device…": "このデバイスの Droppen にサインインを渡しています…",
  "This device has the sign-in.": "このデバイスがサインインを受け取りました。",
  "droppen.app, in the other tab, finishes adding it. You can close this tab and go back to Droppen.":
    "追加はもう一方のタブの droppen.app が完了します。このタブを閉じて Droppen に戻ってください。",
  "Nothing to hand over here. To sign this device in, open Droppen on it: Settings → Access → Droppen → Sign in to Droppen.":
    "ここで渡すものはありません。このデバイスをサインインさせるには、このデバイスで Droppen を開いてください：設定 → アクセス → Droppen → Droppen にサインイン。",
  "This sign-in ran out of time. Start again from Droppen on this device.": "このサインインは時間切れです。このデバイスの Droppen でやり直してください。",
  "This sign-in is no longer open on this device: it was used, cancelled or started again. If it did not finish, start again from Droppen on this device.":
    "このサインインはこのデバイスではもう開いていません（使用済み、キャンセル、またはやり直し済み）。完了していなければ、このデバイスの Droppen でやり直してください。",
  "This answer does not belong to the sign-in open on this device. Nothing was added.": "この応答は、このデバイスで進行中のサインインのものではありません。何も追加されていません。",
  "This device is in a Droppen account already.": "このデバイスはすでに Droppen アカウントに入っています。",
  "This device could not reach droppen.app. Check its connection, then start again from Droppen on this device.":
    "このデバイスから droppen.app に接続できません。接続を確認してから、このデバイスの Droppen でやり直してください。",
  "Droppen on this device did not answer. Is it still running?": "このデバイスの Droppen が応答しません。まだ動いていますか？",
  "This device could not take the sign-in: {{message}}": "このデバイスはサインインを受け取れませんでした：{{message}}",
};
