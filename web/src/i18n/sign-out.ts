/*
 * Signing out of cloud mode (`features/cloud/sign-out-dialog.tsx`): keep this
 * browser as a device of the account, or remove it. Kept apart from the main
 * tables and merged under them in `index.ts`; keys are the English source
 * strings, as everywhere.
 */

export const signOutZhCN: Record<string, string> = {
  "Either way, this browser's offline copies are cleared and its notifications stop.": "无论哪种方式，这个浏览器的离线副本都会被清除，它也不再收到通知。",
  "On a device of your own: this browser stays one of your account's devices, and signing in again opens it as before.":
    "适合你自己的设备：这个浏览器仍是你账户里的设备，再次登录即可照旧使用。",
  "sign out and remove this browser": "退出登录并移除这个浏览器",
  "signing out…": "正在退出登录…",
  "removing this browser…": "正在移除这个浏览器…",
  "On a shared or borrowed device: this browser leaves your account's devices and its keys are deleted here. To use it again, add it as a new device.":
    "适合共用或借来的设备：这个浏览器会离开你账户的设备列表，它在这里的钥匙也会被删除。之后要再用它，需要把它作为新设备添加。",
  "On a shared or borrowed device: its keys are deleted here. It cannot take itself off your account's devices (only a full device can): remove it in my devices on a full device too.":
    "适合共用或借来的设备：它在这里的钥匙会被删除。它不能自己从你账户的设备列表中移除（只有完整设备可以）：请再到一台完整设备的「我的设备」里把它移除。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices, as the account's last full device. Without its keys, only your recovery words bring back a full device.":
    "适合共用或借来的设备：它在这里的钥匙会被删除，但它是账户里最后一台完整设备，所以仍留在设备列表中。没有它的钥匙，只有恢复词能找回一台完整设备。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices: taking it off would also void the recovery words or passkeys it set up. Remove it in my devices on another full device.":
    "适合共用或借来的设备：它在这里的钥匙会被删除，但它仍留在你账户的设备列表中：移除它会让它设置的恢复词或通行密钥一起失效。请在另一台完整设备的「我的设备」里把它移除。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices while a device's activation is held back. Remove it in my devices on another full device.":
    "适合共用或借来的设备：它在这里的钥匙会被删除，但在一台设备的激活被暂缓期间，它仍留在你账户的设备列表中。请在另一台完整设备的「我的设备」里把它移除。",
  "On a shared or borrowed device: its keys are deleted here.": "适合共用或借来的设备：它在这里的钥匙会被删除。",
  "This browser's keys were deleted, but it is still one of your account's devices. Remove it in my devices on another device.":
    "这个浏览器的钥匙已删除，但它仍是你账户里的设备。请在另一台设备的「我的设备」里把它移除。",
  "Sign out on this browser. Its offline copies are cleared and its notifications stop. On a device of your own it stays a device of the account; on a shared one, sign out and remove it.":
    "在这个浏览器上退出登录。它的离线副本会被清除，也不再收到通知。在你自己的设备上，它仍是账户里的设备；在共用的设备上，请退出登录并移除它。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices, as the account's last full device. The account has no recovery words: without its keys, no device can add or remove devices, or become a full device, ever again.":
    "适合共用或借来的设备：它在这里的钥匙会被删除，但它是账户里最后一台完整设备，所以仍留在设备列表中。这个账户没有恢复词：没有它的钥匙，就再也没有设备能添加或移除设备，也没有设备能再成为完整设备。",
  "On a shared or borrowed device: its keys are deleted here. The account could not be opened, so it stays on your account's devices: remove it in my devices on another device.":
    "适合共用或借来的设备：它在这里的钥匙会被删除。账户没能打开，所以它仍留在你账户的设备列表中：请在另一台设备的「我的设备」里把它移除。",
  "This is your account's last full device. Once its keys are deleted, only your recovery words bring back a full device.":
    "这是你账户里最后一台完整设备。它的钥匙一旦删除，只有恢复词能找回一台完整设备。",
  "This is your account's last full device, and the account has no recovery words. Once its keys are deleted, no device can add or remove devices, or become a full device, ever again.":
    "这是你账户里最后一台完整设备，而这个账户没有恢复词。它的钥匙一旦删除，就再也没有设备能添加或移除设备，也没有设备能再成为完整设备。",
  "delete its keys anyway": "仍然删除它的钥匙",
};

export const signOutZhTW: Record<string, string> = {
  "Either way, this browser's offline copies are cleared and its notifications stop.": "無論哪種方式，這個瀏覽器的離線副本都會被清除，它也不再收到通知。",
  "On a device of your own: this browser stays one of your account's devices, and signing in again opens it as before.":
    "適合你自己的裝置：這個瀏覽器仍是你帳戶裡的裝置，再次登入即可照舊使用。",
  "sign out and remove this browser": "登出並移除這個瀏覽器",
  "signing out…": "正在登出…",
  "removing this browser…": "正在移除這個瀏覽器…",
  "On a shared or borrowed device: this browser leaves your account's devices and its keys are deleted here. To use it again, add it as a new device.":
    "適合共用或借來的裝置：這個瀏覽器會離開你帳戶的裝置清單，它在這裡的金鑰也會被刪除。之後要再用它，需要把它作為新裝置加入。",
  "On a shared or borrowed device: its keys are deleted here. It cannot take itself off your account's devices (only a full device can): remove it in my devices on a full device too.":
    "適合共用或借來的裝置：它在這裡的金鑰會被刪除。它不能自己從你帳戶的裝置清單中移除（只有完整裝置可以）：請再到一台完整裝置的「我的裝置」裡把它移除。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices, as the account's last full device. Without its keys, only your recovery words bring back a full device.":
    "適合共用或借來的裝置：它在這裡的金鑰會被刪除，但它是帳戶裡最後一台完整裝置，所以仍留在裝置清單中。沒有它的金鑰，只有復原詞能找回一台完整裝置。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices: taking it off would also void the recovery words or passkeys it set up. Remove it in my devices on another full device.":
    "適合共用或借來的裝置：它在這裡的金鑰會被刪除，但它仍留在你帳戶的裝置清單中：移除它會讓它設定的復原詞或通行金鑰一起失效。請在另一台完整裝置的「我的裝置」裡把它移除。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices while a device's activation is held back. Remove it in my devices on another full device.":
    "適合共用或借來的裝置：它在這裡的金鑰會被刪除，但在一台裝置的啟用被暫緩期間，它仍留在你帳戶的裝置清單中。請在另一台完整裝置的「我的裝置」裡把它移除。",
  "On a shared or borrowed device: its keys are deleted here.": "適合共用或借來的裝置：它在這裡的金鑰會被刪除。",
  "This browser's keys were deleted, but it is still one of your account's devices. Remove it in my devices on another device.":
    "這個瀏覽器的金鑰已刪除，但它仍是你帳戶裡的裝置。請在另一台裝置的「我的裝置」裡把它移除。",
  "Sign out on this browser. Its offline copies are cleared and its notifications stop. On a device of your own it stays a device of the account; on a shared one, sign out and remove it.":
    "在這個瀏覽器上登出。它的離線副本會被清除，也不再收到通知。在你自己的裝置上，它仍是帳戶裡的裝置；在共用的裝置上，請登出並移除它。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices, as the account's last full device. The account has no recovery words: without its keys, no device can add or remove devices, or become a full device, ever again.":
    "適合共用或借來的裝置：它在這裡的金鑰會被刪除，但它是帳戶裡最後一台完整裝置，所以仍留在裝置清單中。這個帳戶沒有復原詞：沒有它的金鑰，就再也沒有裝置能加入或移除裝置，也沒有裝置能再成為完整裝置。",
  "On a shared or borrowed device: its keys are deleted here. The account could not be opened, so it stays on your account's devices: remove it in my devices on another device.":
    "適合共用或借來的裝置：它在這裡的金鑰會被刪除。帳戶沒能開啟，所以它仍留在你帳戶的裝置清單中：請在另一台裝置的「我的裝置」裡把它移除。",
  "This is your account's last full device. Once its keys are deleted, only your recovery words bring back a full device.":
    "這是你帳戶裡最後一台完整裝置。它的金鑰一旦刪除，只有復原詞能找回一台完整裝置。",
  "This is your account's last full device, and the account has no recovery words. Once its keys are deleted, no device can add or remove devices, or become a full device, ever again.":
    "這是你帳戶裡最後一台完整裝置，而這個帳戶沒有復原詞。它的金鑰一旦刪除，就再也沒有裝置能加入或移除裝置，也沒有裝置能再成為完整裝置。",
  "delete its keys anyway": "仍然刪除它的金鑰",
};

export const signOutJa: Record<string, string> = {
  "Either way, this browser's offline copies are cleared and its notifications stop.": "どちらを選んでも、このブラウザのオフラインコピーは消去され、通知も届かなくなります。",
  "On a device of your own: this browser stays one of your account's devices, and signing in again opens it as before.":
    "自分のデバイス向け：このブラウザはアカウントのデバイスのままで、再度ログインすれば元どおり使えます。",
  "sign out and remove this browser": "ログアウトしてこのブラウザを削除",
  "signing out…": "ログアウトしています…",
  "removing this browser…": "このブラウザを削除しています…",
  "On a shared or borrowed device: this browser leaves your account's devices and its keys are deleted here. To use it again, add it as a new device.":
    "共用や借りたデバイス向け：このブラウザはアカウントのデバイスから外れ、ここにある鍵も削除されます。もう一度使うには、新しいデバイスとして追加してください。",
  "On a shared or borrowed device: its keys are deleted here. It cannot take itself off your account's devices (only a full device can): remove it in my devices on a full device too.":
    "共用や借りたデバイス向け：ここにある鍵は削除されます。このブラウザは自分でアカウントのデバイスから外れることができません（できるのはフルデバイスだけです）。フルデバイスの「マイデバイス」でも削除してください。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices, as the account's last full device. Without its keys, only your recovery words bring back a full device.":
    "共用や借りたデバイス向け：ここにある鍵は削除されますが、アカウント最後のフルデバイスなので、デバイスの一覧には残ります。その鍵がなければ、フルデバイスを取り戻せるのはリカバリーワードだけです。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices: taking it off would also void the recovery words or passkeys it set up. Remove it in my devices on another full device.":
    "共用や借りたデバイス向け：ここにある鍵は削除されますが、デバイスの一覧には残ります。外すと、このブラウザが設定したリカバリーワードやパスキーも無効になるためです。別のフルデバイスの「マイデバイス」で削除してください。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices while a device's activation is held back. Remove it in my devices on another full device.":
    "共用や借りたデバイス向け：ここにある鍵は削除されますが、デバイスの有効化が保留されている間は、デバイスの一覧に残ります。別のフルデバイスの「マイデバイス」で削除してください。",
  "On a shared or borrowed device: its keys are deleted here.": "共用や借りたデバイス向け：ここにある鍵は削除されます。",
  "This browser's keys were deleted, but it is still one of your account's devices. Remove it in my devices on another device.":
    "このブラウザの鍵は削除されましたが、まだアカウントのデバイスです。別のデバイスの「マイデバイス」で削除してください。",
  "Sign out on this browser. Its offline copies are cleared and its notifications stop. On a device of your own it stays a device of the account; on a shared one, sign out and remove it.":
    "このブラウザでログアウトします。オフラインコピーは消去され、通知も届かなくなります。自分のデバイスならアカウントのデバイスのままです。共用のデバイスでは、ログアウトしてこのブラウザを削除してください。",
  "On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices, as the account's last full device. The account has no recovery words: without its keys, no device can add or remove devices, or become a full device, ever again.":
    "共用や借りたデバイス向け：ここにある鍵は削除されますが、アカウント最後のフルデバイスなので、デバイスの一覧には残ります。このアカウントにはリカバリーワードがありません。その鍵がなければ、デバイスを追加・削除できるデバイスも、フルデバイスになれるデバイスも、二度となくなります。",
  "On a shared or borrowed device: its keys are deleted here. The account could not be opened, so it stays on your account's devices: remove it in my devices on another device.":
    "共用や借りたデバイス向け：ここにある鍵は削除されます。アカウントを開けなかったため、デバイスの一覧には残ります。別のデバイスの「マイデバイス」で削除してください。",
  "This is your account's last full device. Once its keys are deleted, only your recovery words bring back a full device.":
    "これはアカウント最後のフルデバイスです。鍵を削除すると、フルデバイスを取り戻せるのはリカバリーワードだけです。",
  "This is your account's last full device, and the account has no recovery words. Once its keys are deleted, no device can add or remove devices, or become a full device, ever again.":
    "これはアカウント最後のフルデバイスで、このアカウントにはリカバリーワードがありません。鍵を削除すると、デバイスを追加・削除できるデバイスも、フルデバイスになれるデバイスも、二度となくなります。",
  "delete its keys anyway": "それでも鍵を削除",
};
