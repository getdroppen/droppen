// The ja table of `setup.ts`, which says what it covers: a file of its
// own, so that it is in the ja chunk alone (`lang-ja.ts`).

export const setupJa: Record<string, string> = {
  "setup code": "セットアップコード",
  "confirm password": "パスワード（確認）",
  "create account and sign in": "アカウントを作成してログイン",
  "creating account…": "アカウントを作成中…",
  "Or create the account on this device, without a code:": "または、コードなしでこのデバイス上でアカウントを作成します：",
  "a setup code is 16 letters and digits, like ABCD-EFGH-JKMN-PQRS": "セットアップコードは 16 文字の英数字です（例：ABCD-EFGH-JKMN-PQRS）",
  "a user name is 1-32 characters of a-z, 0-9, '.', '_' or '-'": "ユーザー名は a-z、0-9、「.」「_」「-」からなる 1〜32 文字です",
  "a password is at least 12 characters long": "パスワードは 12 文字以上にしてください",
  "a password is at most 256 bytes long": "パスワードは 256 バイト以内にしてください",
  "a password may not start or end with whitespace": "パスワードの先頭と末尾に空白は使えません",
  "a password may not be the user name": "パスワードをユーザー名と同じにはできません",
  "that password is one of the most common ones; pick another": "よく使われているパスワードです。別のものにしてください",
  "That is not the setup code this device's server printed. `{{command}} setup-code` on this device shows it again.":
    "このデバイスのサーバーが表示したセットアップコードではありません。このデバイスで `{{command}} setup-code` を実行すると、もう一度表示されます。",
  "The first account is made on this device's own page, not through Droppen.": "最初のアカウントは Droppen 経由ではなく、このデバイス自身のページで作成します。",
  "This device already has an account.": "このデバイスにはすでにアカウントがあります。",
  "Local login is switched off on this device (auth.enabled = false), so there is no account to create here.":
    "このデバイスではローカルログインがオフ（auth.enabled = false）のため、ここで作成するアカウントはありません。",
  "This device's server has no setup code. Restart it to get one, or create the account on this device with `{{command}} user add <name>`.":
    "このデバイスのサーバーにはセットアップコードがありません。サーバーを再起動するとコードが発行されます。または、このデバイスで `{{command}} user add <名前>` を実行してアカウントを作成してください。",
  "Too many wrong setup codes. Try again in {{seconds}} s.": "セットアップコードの誤りが多すぎます。{{seconds}} 秒後にもう一度お試しください。",
  "Too many wrong setup codes. Wait a little, then try again.": "セットアップコードの誤りが多すぎます。少し待ってからもう一度お試しください。",
  /* settings › security, when local login is switched off */
  "switched off": "オフ",
  "Local login is switched off on this device (auth.enabled = false), so its page answers anyone who can reach it. To require a login, remove this setting from the server's configuration and restart the server:":
    "このデバイスではローカルログインがオフ（auth.enabled = false）のため、到達できる相手なら誰でもこのページを使えます。ログインを必須にするには、サーバーの設定からこの項目を削除して、サーバーを再起動してください：",
  "Then make its first account on this device:": "次に、このデバイスで最初のアカウントを作成します：",
  "Could not ask this device's server about its login.": "このデバイスのサーバーにログインの状態を問い合わせられませんでした。",
  /* settings › security, this device's owner while it has no account */
  "no account yet": "アカウントはまだありません",
  /* the setup page: nobody signed in, no account yet */
  "This device has no account yet.": "このデバイスにはまだアカウントがありません。",
  "This is the Droppen app's own server. In the app's menu-bar menu, choose Open in Browser: it opens this page signed in, and the first account is made there.":
    "これは Droppen アプリ自身のサーバーです。アプリのメニューバーのメニューで「ブラウザで開く」を選ぶと、ログインした状態でこのページが開きます。最初のアカウントはそこで作成します。",
  "On this device: open Droppen from the app, or run `{{command}} open` in a terminal — it opens this page signed in.":
    "このデバイスで、アプリから Droppen を開くか、ターミナルで `{{command}} open` を実行してください。ログインした状態でこのページが開きます。",
  "Or create the first account with the one-time setup code this device's server keeps (`{{command}} setup-code` on this device shows it)":
    "または、このデバイスのサーバーが保存しているワンタイムのセットアップコードで最初のアカウントを作成します（このデバイスで `{{command}} setup-code` を実行すると表示されます）",
  "That link was already used or has expired. Run `{{command}} open` on this device again.":
    "このリンクは使用済みか、期限切れです。このデバイスでもう一度 `{{command}} open` を実行してください。",
  "This device has an account now: sign in.": "このデバイスにはもうアカウントがあります。ログインしてください。",
  "That link works only on the device itself, not through Droppen.": "このリンクは Droppen 経由では使えず、このデバイス上でのみ使えます。",
  /* the first account, made by this device's owner without the code */
  "You opened this page as this device's owner. This device has no account yet: create one to sign in from other devices; from then on every browser signs in, this one too.":
    "このデバイスの所有者としてこのページを開いています。このデバイスにはまだアカウントがありません。作成すると他のデバイスからログインできるようになり、それ以降はこのブラウザも含め、すべてのブラウザでログインが必要になります。",
  "The setup code is needed now. `{{command}} setup-code` on this device shows it, or run `{{command}} open` there again.":
    "セットアップコードが必要になりました。このデバイスで `{{command}} setup-code` を実行すると表示されます。または、そこでもう一度 `{{command}} open` を実行してください。",
  "That name belongs to the Droppen app's own account on this device. Pick another.": "その名前は、このデバイス上の Droppen アプリ自身のアカウントが使っています。別の名前にしてください。",
};
