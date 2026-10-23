/*
 * The move notice on the hub's origin once the app has moved to its app host
 * (`features/hosts/legacy.tsx`: droppen.app → my.droppen.app). Kept apart
 * from the main tables and merged under them in `index.ts`; keys are the
 * English source strings, as everywhere. The notice's removal reuses the
 * sign-out's words (`sign-out.ts`).
 */

export const hostsZhCN: Record<string, string> = {
  "Droppen moved to {{host}}": "Droppen 已搬到 {{host}}",
  "This window was opened by Droppen on one of your devices to sign it in, and it cannot finish here any more. Close it, then pick “continue in a browser tab instead” in Droppen. If Droppen does not offer that, update Droppen on that device, or add it with a one-time code from {{host}} (my devices → add a device that runs AI).":
    "这个窗口是你某台设备上的 Droppen 为登录它而打开的，现在已无法在这里完成。请关闭它，然后在 Droppen 里选择「改在浏览器标签页里继续」。如果 Droppen 没有这个选项，请在那台设备上更新 Droppen，或者在 {{host}} 上用一次性注册码添加它（「我的设备 → 添加可运行 AI 的设备」）。",
  "This browser is set up as {{device}} for {{email}}, here at {{apex}}.": "这个浏览器在 {{apex}} 上是 {{email}} 的设备「{{device}}」。",
  "This browser is set up as a device of {{email}}'s account, here at {{apex}}.": "这个浏览器在 {{apex}} 上是 {{email}} 账户的一台设备。",
  "This browser is set up as a device of your Droppen account, here at {{apex}}.": "这个浏览器在 {{apex}} 上是你 Droppen 账户的一台设备。",
  "This app was added to the Home Screen from {{apex}}, and its keys cannot move to {{host}}. Remove this browser here, then open {{host}} in Safari, add it to the Home Screen, and add it to your account as a new device.":
    "这个应用是从 {{apex}} 添加到主屏幕的，它的钥匙无法搬到 {{host}}。请先在这里移除这个浏览器，然后在 Safari 中打开 {{host}}，添加到主屏幕，再把它作为新设备加入你的账户。",
  "Move this browser": "搬移这个浏览器",
  "Recommended. A small {{host}} window opens, takes this browser's keys and closes; this tab then carries on at {{host}}.":
    "推荐。会打开一个 {{host}} 小窗口，接过这个浏览器的钥匙后自动关闭；这个标签页随后在 {{host}} 上继续。",
  "Remove this browser instead": "改为移除这个浏览器",
  "Remove this browser": "移除这个浏览器",
  "Takes this browser off your account's devices where it can, and deletes its keys here.": "在可以的情况下把这个浏览器从你账户的设备列表中移除，并删除它在这里的钥匙。",
  "Continue without moving": "不搬移，继续",
  "Its keys stay here for now: {{host}} can still bring them over later.": "它的钥匙暂时留在这里：之后仍可以在 {{host}} 上把它们接过去。",
  "Its keys stay here. At {{host}}, sign in and add this browser as a new device, then remove this one in my devices.":
    "它的钥匙留在这里。请在 {{host}} 登录，把这个浏览器作为新设备加入，再在「我的设备」里移除这一台。",
  "moving this browser…": "正在搬移这个浏览器…",
  "opening your account…": "正在打开你的账户…",
  "The browser blocked the {{host}} window. Allow pop-ups for {{apex}} and try again.": "浏览器拦截了 {{host}} 的窗口。请允许 {{apex}} 的弹出窗口后再试一次。",
  "Moving did not go through, and nothing was deleted here: {{message}}": "搬移没有完成，这里没有删除任何东西：{{message}}",
  "This browser cannot move its keys. Remove it here, or continue and add it at {{host}} as a new device.":
    "这个浏览器无法搬移它的钥匙。请在这里移除它，或者继续，在 {{host}} 上把它作为新设备加入。",
  "This browser is now at {{host}}. Close this window, then pick “continue in a browser tab instead” in Droppen.":
    "这个浏览器现在已在 {{host}}。请关闭这个窗口，然后在 Droppen 里选择「改在浏览器标签页里继续」。",
  "This browser's keys are deleted here.": "这个浏览器在这里的钥匙已删除。",
  "Some of it could not be cleared:": "有些内容没能清除：",
  "Open {{host}} in Safari and add it to the Home Screen: Droppen is there now.": "请在 Safari 中打开 {{host}} 并添加到主屏幕：Droppen 现在在那里。",
  "open {{host}}": "打开 {{host}}",
  "Continue to {{host}}": "前往 {{host}}",
};

export const hostsZhTW: Record<string, string> = {
  "Droppen moved to {{host}}": "Droppen 已搬到 {{host}}",
  "This window was opened by Droppen on one of your devices to sign it in, and it cannot finish here any more. Close it, then pick “continue in a browser tab instead” in Droppen. If Droppen does not offer that, update Droppen on that device, or add it with a one-time code from {{host}} (my devices → add a device that runs AI).":
    "這個視窗是你某台裝置上的 Droppen 為登入它而開啟的，現在已無法在這裡完成。請關閉它，然後在 Droppen 裡選擇「改在瀏覽器分頁裡繼續」。如果 Droppen 沒有這個選項，請在那台裝置上更新 Droppen，或者在 {{host}} 上用一次性註冊碼新增它（「我的裝置 → 新增可執行 AI 的裝置」）。",
  "This browser is set up as {{device}} for {{email}}, here at {{apex}}.": "這個瀏覽器在 {{apex}} 上是 {{email}} 的裝置「{{device}}」。",
  "This browser is set up as a device of {{email}}'s account, here at {{apex}}.": "這個瀏覽器在 {{apex}} 上是 {{email}} 帳戶的一台裝置。",
  "This browser is set up as a device of your Droppen account, here at {{apex}}.": "這個瀏覽器在 {{apex}} 上是你 Droppen 帳戶的一台裝置。",
  "This app was added to the Home Screen from {{apex}}, and its keys cannot move to {{host}}. Remove this browser here, then open {{host}} in Safari, add it to the Home Screen, and add it to your account as a new device.":
    "這個 App 是從 {{apex}} 加入主畫面的，它的金鑰無法搬到 {{host}}。請先在這裡移除這個瀏覽器，然後在 Safari 中開啟 {{host}}，加入主畫面，再把它作為新裝置加入你的帳戶。",
  "Move this browser": "搬移這個瀏覽器",
  "Recommended. A small {{host}} window opens, takes this browser's keys and closes; this tab then carries on at {{host}}.":
    "建議。會開啟一個 {{host}} 小視窗，接過這個瀏覽器的金鑰後自動關閉；這個分頁隨後在 {{host}} 上繼續。",
  "Remove this browser instead": "改為移除這個瀏覽器",
  "Remove this browser": "移除這個瀏覽器",
  "Takes this browser off your account's devices where it can, and deletes its keys here.": "在可以的情況下把這個瀏覽器從你帳戶的裝置清單中移除，並刪除它在這裡的金鑰。",
  "Continue without moving": "不搬移，繼續",
  "Its keys stay here for now: {{host}} can still bring them over later.": "它的金鑰暫時留在這裡：之後仍可以在 {{host}} 上把它們接過去。",
  "Its keys stay here. At {{host}}, sign in and add this browser as a new device, then remove this one in my devices.":
    "它的金鑰留在這裡。請在 {{host}} 登入，把這個瀏覽器作為新裝置加入，再在「我的裝置」裡移除這一台。",
  "moving this browser…": "正在搬移這個瀏覽器…",
  "opening your account…": "正在開啟你的帳戶…",
  "The browser blocked the {{host}} window. Allow pop-ups for {{apex}} and try again.": "瀏覽器封鎖了 {{host}} 的視窗。請允許 {{apex}} 的彈出式視窗後再試一次。",
  "Moving did not go through, and nothing was deleted here: {{message}}": "搬移沒有完成，這裡沒有刪除任何東西：{{message}}",
  "This browser cannot move its keys. Remove it here, or continue and add it at {{host}} as a new device.":
    "這個瀏覽器無法搬移它的金鑰。請在這裡移除它，或者繼續，在 {{host}} 上把它作為新裝置加入。",
  "This browser is now at {{host}}. Close this window, then pick “continue in a browser tab instead” in Droppen.":
    "這個瀏覽器現在已在 {{host}}。請關閉這個視窗，然後在 Droppen 裡選擇「改在瀏覽器分頁裡繼續」。",
  "This browser's keys are deleted here.": "這個瀏覽器在這裡的金鑰已刪除。",
  "Some of it could not be cleared:": "有些內容沒能清除：",
  "Open {{host}} in Safari and add it to the Home Screen: Droppen is there now.": "請在 Safari 中開啟 {{host}} 並加入主畫面：Droppen 現在在那裡。",
  "open {{host}}": "開啟 {{host}}",
  "Continue to {{host}}": "前往 {{host}}",
};

export const hostsJa: Record<string, string> = {
  "Droppen moved to {{host}}": "Droppen は {{host}} に移りました",
  "This window was opened by Droppen on one of your devices to sign it in, and it cannot finish here any more. Close it, then pick “continue in a browser tab instead” in Droppen. If Droppen does not offer that, update Droppen on that device, or add it with a one-time code from {{host}} (my devices → add a device that runs AI).":
    "このウィンドウは、あなたのデバイスの Droppen がそのデバイスをログインさせるために開いたものですが、ここではもう完了できません。ウィンドウを閉じて、Droppen で「代わりにブラウザのタブで続ける」を選んでください。その項目がない場合は、そのデバイスの Droppen をアップデートするか、{{host}} のワンタイムコードで追加してください（「マイデバイス → AI を実行できるデバイスを追加」）。",
  "This browser is set up as {{device}} for {{email}}, here at {{apex}}.": "このブラウザは {{apex}} で、{{email}} のデバイス「{{device}}」として設定されています。",
  "This browser is set up as a device of {{email}}'s account, here at {{apex}}.": "このブラウザは {{apex}} で、{{email}} のアカウントのデバイスとして設定されています。",
  "This browser is set up as a device of your Droppen account, here at {{apex}}.": "このブラウザは {{apex}} で、あなたの Droppen アカウントのデバイスとして設定されています。",
  "This app was added to the Home Screen from {{apex}}, and its keys cannot move to {{host}}. Remove this browser here, then open {{host}} in Safari, add it to the Home Screen, and add it to your account as a new device.":
    "このアプリは {{apex}} からホーム画面に追加されたもので、その鍵は {{host}} に移せません。ここでこのブラウザを削除してから、Safari で {{host}} を開いてホーム画面に追加し、新しいデバイスとしてアカウントに追加してください。",
  "Move this browser": "このブラウザを移す",
  "Recommended. A small {{host}} window opens, takes this browser's keys and closes; this tab then carries on at {{host}}.":
    "おすすめ。{{host}} の小さなウィンドウが開き、このブラウザの鍵を受け取って閉じます。このタブはそのまま {{host}} で続きます。",
  "Remove this browser instead": "代わりにこのブラウザを削除",
  "Remove this browser": "このブラウザを削除",
  "Takes this browser off your account's devices where it can, and deletes its keys here.": "できる場合はこのブラウザをアカウントのデバイスから外し、ここにある鍵を削除します。",
  "Continue without moving": "移さずに続ける",
  "Its keys stay here for now: {{host}} can still bring them over later.": "鍵はひとまずここに残ります。あとから {{host}} で引き継ぐこともできます。",
  "Its keys stay here. At {{host}}, sign in and add this browser as a new device, then remove this one in my devices.":
    "鍵はここに残ります。{{host}} でログインしてこのブラウザを新しいデバイスとして追加し、「マイデバイス」でこちらを削除してください。",
  "moving this browser…": "このブラウザを移しています…",
  "opening your account…": "アカウントを開いています…",
  "The browser blocked the {{host}} window. Allow pop-ups for {{apex}} and try again.": "ブラウザが {{host}} のウィンドウをブロックしました。{{apex}} のポップアップを許可して、もう一度お試しください。",
  "Moving did not go through, and nothing was deleted here: {{message}}": "移せませんでした。ここでは何も削除されていません：{{message}}",
  "This browser cannot move its keys. Remove it here, or continue and add it at {{host}} as a new device.":
    "このブラウザは鍵を移せません。ここで削除するか、続けて {{host}} で新しいデバイスとして追加してください。",
  "This browser is now at {{host}}. Close this window, then pick “continue in a browser tab instead” in Droppen.":
    "このブラウザは {{host}} に移りました。このウィンドウを閉じて、Droppen で「代わりにブラウザのタブで続ける」を選んでください。",
  "This browser's keys are deleted here.": "このブラウザのここにある鍵は削除されました。",
  "Some of it could not be cleared:": "消去できなかったものがあります：",
  "Open {{host}} in Safari and add it to the Home Screen: Droppen is there now.": "Safari で {{host}} を開いてホーム画面に追加してください。Droppen は今そこにあります。",
  "open {{host}}": "{{host}} を開く",
  "Continue to {{host}}": "{{host}} へ進む",
};
