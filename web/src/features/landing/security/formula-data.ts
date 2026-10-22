/*
 * The formulas of droppen.app/security, part by part (`formula.tsx` draws
 * them). Every part of every formula is a term: what kind of thing it is,
 * which party can know it, one or two lines on what it is and why it is
 * there (en, zh-CN, zh-TW, ja), and the code it was read from (`src`, a
 * path from the repository root with its lines) — so a reviewer can check
 * each claim, and `test/security-formulas.test.tsx` checks that every token
 * of every formula has all of it.
 *
 * The parties ("view as"): `me` is your device and `other` the device at the
 * other end, each block saying which role is which (the sender and the
 * recipient of a sealed key, the reader and the device that runs AI that
 * writes a stream…); `hub` is Droppen Hub, `relay` a Droppen relay. A party
 * "knows" a value when the protocol hands it the value or everything needed
 * to compute it: the Hub stores and serves the chain, the mailbox and the
 * encrypted streams (chain.rs: "The hub stores and serves the chain"; wrap.rs;
 * record.rs: "The hub stores what it is handed"); a relay holds only tickets
 * (account, ids, Ed25519 public keys: ticket.rs) and copies channel frames;
 * labels, constants, functions and operators are public, in the source.
 *
 * The same symbol can mean different things in different places — `e` in
 * `X25519(e, R)` is the one-off private key, in `info = e ‖ …` its public
 * half — so a block or a line maps a symbol per occurrence where it must.
 */

export const LANGS = ["en", "zh-CN", "zh-TW", "ja"] as const;
export type Lang = (typeof LANGS)[number];
/** One text in every language: en, zh-CN, zh-TW, ja. */
export type Words = readonly [en: string, zhCN: string, zhTW: string, ja: string];

/** What a token is; each kind is drawn its own way (`formula.tsx`). */
export const KINDS = ["secret", "derived", "public", "value", "label", "fn", "op"] as const;
export type Kind = (typeof KINDS)[number];

export const PARTIES = ["me", "other", "hub", "relay"] as const;
export type Party = (typeof PARTIES)[number];

export interface Term {
  kind: Kind;
  /** Who can know this value (`label`, `fn` and `op`: everyone). */
  known: readonly Party[];
  /** What it is, and why it is there. */
  says: Words;
  /** Other terms to light up with it: its other half, the value it names. */
  rel?: readonly string[];
  /** Where it was read from, from the repository root. */
  src: string;
}

const ALL: readonly Party[] = ["me", "other", "hub", "relay"];
/** The two ends only. */
const ENDS: readonly Party[] = ["me", "other"];
/** The two ends and the Hub, which stores it or carries it in the clear. */
const STORED: readonly Party[] = ["me", "other", "hub"];
const ME: readonly Party[] = ["me"];
const OTHER: readonly Party[] = ["other"];

const P = "server-rs/crates/rh-proto/src";

export const TERMS: Record<string, Term> = {
  // ---- functions
  "fn.ed25519": {
    kind: "fn",
    known: ALL,
    says: [
      "Signature with an Ed25519 private key; anyone with the public key can check it. Checked strictly: no malleable signatures, no small-order keys.",
      "用 Ed25519 私钥签名，有公钥的人都能验。验得很严：不收可变形的签名，不收小阶公钥。",
      "用 Ed25519 私鑰簽章，有公鑰的人都能驗。驗得很嚴：不收可變形的簽章，不收小階公鑰。",
      "Ed25519 の秘密鍵による署名。公開鍵があれば誰でも検証できる。検証は厳格で、改変可能な署名も小位数の鍵も受け付けない。",
    ],
    src: `${P}/keys.rs:99-112`,
  },
  "fn.sha256": {
    kind: "fn",
    known: ALL,
    says: [
      "Hash: a 32-byte fingerprint of its input. Change one byte of the input and it no longer matches.",
      "哈希：输入的 32 字节指纹。输入改动一个字节，它就对不上了。",
      "雜湊：輸入的 32 位元組指紋。輸入改動一個位元組，它就對不上了。",
      "ハッシュ：入力の 32 バイトの指紋。入力が 1 バイトでも変われば一致しなくなる。",
    ],
    src: `${P}/chain.rs:1331-1337`,
  },
  "fn.hkdf": {
    kind: "fn",
    known: ALL,
    says: [
      "Key derivation: turns a secret into a key of its own, kept apart by salt and info, so no two uses ever share a key.",
      "密钥派生：把一份秘密变成一把专用的钥，用 salt 和 info 区分用途，所以两种用途永远不会共用一把钥。",
      "金鑰衍生：把一份秘密變成一把專用的金鑰，用 salt 和 info 區分用途，所以兩種用途永遠不會共用一把金鑰。",
      "鍵導出：秘密から専用の鍵を作る。salt と info で用途を分けるので、2 つの用途が同じ鍵を使うことはない。",
    ],
    src: `${P}/wrap.rs:157-163`,
  },
  "fn.x25519": {
    kind: "fn",
    known: ALL,
    says: [
      "Diffie–Hellman: one side's private key with the other's public key gives a secret both sides can compute. Small-order keys are refused.",
      "Diffie–Hellman：一方的私钥配另一方的公钥，得出一份双方都算得出的秘密。小阶公钥一律拒绝。",
      "Diffie–Hellman：一方的私鑰配另一方的公鑰，得出一份雙方都算得出的秘密。小階公鑰一律拒絕。",
      "Diffie–Hellman：片方の秘密鍵と相手の公開鍵から、双方が計算できる共有の秘密を得る。小位数の鍵は拒否する。",
    ],
    src: `${P}/keys.rs:50-55`,
  },
  "fn.aesgcm": {
    kind: "fn",
    known: ALL,
    says: [
      "Authenticated encryption: hides the content, and refuses to decrypt if one byte of it, or of its AAD, was changed.",
      "认证加密：藏起内容；内容或 AAD 只要改了一个字节，就拒绝解密。",
      "認證加密：藏起內容；內容或 AAD 只要改了一個位元組，就拒絕解密。",
      "認証付き暗号：内容を隠し、内容か AAD が 1 バイトでも変えられていれば復号を拒む。",
    ],
    src: `${P}/wrap.rs:165-178`,
  },
  "fn.hmac": {
    kind: "fn",
    known: ALL,
    says: [
      "Keyed hash (MAC): only someone with the key can compute it, so a correct MAC proves the key is held.",
      "带钥的哈希（MAC）：只有拿着钥的人算得出，所以 MAC 对得上就证明对方握着这把钥。",
      "帶金鑰的雜湊（MAC）：只有拿著金鑰的人算得出，所以 MAC 對得上就證明對方握著這把金鑰。",
      "鍵付きハッシュ（MAC）：鍵を持つ者にしか計算できない。正しい MAC はその鍵を持っている証明になる。",
    ],
    src: `${P}/chain.rs:376-387`,
  },
  "fn.base32": {
    kind: "fn",
    known: ALL,
    says: [
      "RFC 4648 base32 in capitals: letters A–Z and digits 2–7, easy to read and to type.",
      "RFC 4648 的 base32，大写：字母 A–Z 和数字 2–7，好认也好输入。",
      "RFC 4648 的 base32，大寫：字母 A–Z 和數字 2–7，好認也好輸入。",
      "RFC 4648 の base32（大文字）：A–Z と 2–7 だけで、読みやすく打ちやすい。",
    ],
    src: `${P}/codec.rs:138-141`,
  },

  // ---- operators
  "op.concat": {
    kind: "op",
    known: ALL,
    says: [
      "Concatenation: the bytes on the left, then those on the right. Each part has a fixed length or follows a label ending in NUL, so it splits back one way only.",
      "拼接：左边的字节后面紧跟右边的字节。每一段要么定长，要么跟在以 NUL 结尾的标签后面，所以只有一种拆法。",
      "串接：左邊的位元組後面緊跟右邊的位元組。每一段要麼定長，要麼跟在以 NUL 結尾的標籤後面，所以只有一種拆法。",
      "連結：左のバイト列のすぐ後に右のバイト列。各部分は固定長か、NUL で終わるラベルの後に来るので、分け方は 1 通りしかない。",
    ],
    src: `${P}/sig.rs:72-75`,
  },
  "op.field": {
    kind: "op",
    known: ALL,
    says: [
      "Next field of a fixed binary layout: numbers big-endian, ids after a 2-byte length, keys, hashes and signatures as raw bytes of a fixed size. One reading only.",
      "固定二进制结构里的下一个字段：数字按大端序，id 前面带 2 字节长度，钥、哈希和签名是定长的原样字节。只有一种读法。",
      "固定二進位結構裡的下一個欄位：數字按大端序，id 前面帶 2 位元組長度，金鑰、雜湊和簽章是定長的原樣位元組。只有一種讀法。",
      "固定のバイナリ構造の次のフィールド：数値はビッグエンディアン、id は 2 バイトの長さの後、鍵・ハッシュ・署名は固定長の生のバイト列。読み方は 1 通りしかない。",
    ],
    src: `${P}/codec.rs:4-8`,
  },
  "op.signed": {
    kind: "op",
    known: ALL,
    says: [
      "Signed by: both entries go onto the chain together, each signed by a full device under the chain-entry label.",
      "由它签名：两条条目一起写上链，每条都由完整设备在 chain-entry 标签下签名。",
      "由它簽章：兩筆項目一起寫上鏈，每筆都由完整裝置在 chain-entry 標籤下簽章。",
      "署名者：2 つのエントリは一緒にチェーンへ載り、どちらもフルデバイスが chain-entry ラベルで署名する。",
    ],
    src: `${P}/chain.rs:1219-1222`,
  },
  "op.slice": {
    kind: "op",
    known: ALL,
    says: [
      "The first 5 bytes (40 bits) of the hash, which base32 writes as 8 characters.",
      "哈希的前 5 个字节（40 位），base32 写出来正好 8 个字符。",
      "雜湊的前 5 個位元組（40 位元），base32 寫出來正好 8 個字元。",
      "ハッシュの先頭 5 バイト（40 ビット）。base32 で 8 文字になる。",
    ],
    src: `${P}/chain.rs:407-418`,
  },
  "op.as": {
    kind: "op",
    known: ALL,
    says: ["Shown as two groups of four characters.", "显示成两组，每组四个字符。", "顯示成兩組，每組四個字元。", "4 文字ずつ 2 組に分けて表示する。"],
    src: `${P}/chain.rs:407-418`,
  },

  // ---- ids, the same everywhere
  "id.account": {
    kind: "value",
    known: ALL,
    says: [
      "Your account's random id. Every chain entry, attestation, seal context, key commitment, prologue and renewal message names it, so none of them passes in another account.",
      "你账户的随机 id。每条链条目、作证、封装上下文、钥匙承诺、prologue 和更换消息都写着它，所以拿到别的账户都不成立。",
      "你帳戶的隨機 id。每筆鏈項目、作證、封裝上下文、金鑰承諾、prologue 和更換訊息都寫著它，所以拿到別的帳戶都不成立。",
      "アカウントのランダムな id。チェーンのエントリ、証明、封印コンテキスト、鍵のコミットメント、prologue、更新のメッセージのすべてに入っているので、どれも別のアカウントでは通らない。",
    ],
    src: `${P}/chain.rs:1140`,
  },
  "id.device": {
    kind: "value",
    known: ALL,
    says: [
      "The random id of your device (a browser or phone) on the chain. It says whose this is, so it cannot be passed off as another device's.",
      "你的设备（一个浏览器或一部手机）在链上的随机 id。它表明这是谁的，所以没法冒充成另一台设备的。",
      "你的裝置（一個瀏覽器或一支手機）在鏈上的隨機 id。它表明這是誰的，所以沒辦法冒充成另一台裝置的。",
      "チェーン上のあなたのデバイス（ブラウザーやスマートフォン）のランダムな id。誰のものかを示すので、別のデバイスのものとして通すことはできない。",
    ],
    src: `${P}/noise.rs:112-135`,
  },
  "id.host": {
    kind: "value",
    known: ALL,
    says: [
      "The random id of the device that runs AI. It binds this to that one device, so it cannot be used for another.",
      "可运行 AI 的设备的随机 id。把这份东西绑在这一台设备上，拿去别的设备就不成立。",
      "可執行 AI 的裝置的隨機 id。把這份東西綁在這一台裝置上，拿去別的裝置就不成立。",
      "AI を実行できるデバイスのランダムな id。これをその 1 台に結び付けるので、別のデバイスには使えない。",
    ],
    src: `${P}/noise.rs:112-135`,
  },

  // ---- labels
  "label.chain-entry": {
    kind: "label",
    known: ALL,
    says: [
      "Label in front of every chain entry's signature. Each kind of signature has its own label ending in NUL, so none can pass for another.",
      "每条链条目签名前面的标签。每种签名都有自己以 NUL 结尾的标签，所以谁也冒充不了谁。",
      "每筆鏈項目簽章前面的標籤。每種簽章都有自己以 NUL 結尾的標籤，所以誰也冒充不了誰。",
      "チェーンのエントリの署名の前に付くラベル。署名の種類ごとに NUL で終わる固有のラベルがあるので、互いに成りすませない。",
    ],
    src: `${P}/sig.rs:24-33`,
  },
  "label.chain-hash": {
    kind: "label",
    known: ALL,
    says: [
      "Label in front of every entry hash, so an entry's hash can never equal a hash made for anything else.",
      "每个条目哈希前面的标签，所以条目的哈希永远不会等于为别的东西算的哈希。",
      "每個項目雜湊前面的標籤，所以項目的雜湊永遠不會等於為別的東西算的雜湊。",
      "エントリのハッシュの前に付くラベル。エントリのハッシュがほかの何かのハッシュと一致することはない。",
    ],
    src: `${P}/chain.rs:118-119`,
  },
  "label.chain-attest": {
    kind: "label",
    known: ALL,
    says: [
      "Label for attestations: a full device's word on the chain head it sees.",
      "作证用的标签：完整设备对它所见链头的表态。",
      "作證用的標籤：完整裝置對它所見鏈頭的表態。",
      "証明用のラベル：フルデバイスが自分の見ているチェーンの先頭を保証するもの。",
    ],
    src: `${P}/sig.rs:11-34`,
  },
  "label.seal-auth": {
    kind: "label",
    known: ALL,
    says: [
      "HKDF salt for keys sealed from one device to another; every other use of HKDF has a salt of its own.",
      "设备之间封装钥匙用的 HKDF salt；HKDF 的其他用途各有各的 salt。",
      "裝置之間封裝金鑰用的 HKDF salt；HKDF 的其他用途各有各的 salt。",
      "デバイス間で鍵を封印するときの HKDF の salt。HKDF のほかの用途はそれぞれ別の salt を使う。",
    ],
    src: `${P}/wrap.rs:45`,
  },
  "label.seal": {
    kind: "label",
    known: ALL,
    says: [
      "Starts the seal context: protocol version 1, and what these bytes are.",
      "封装上下文的开头：协议第 1 版，以及这些字节是什么。",
      "封裝上下文的開頭：協定第 1 版，以及這些位元組是什麼。",
      "封印コンテキストの先頭：プロトコルの第 1 版であることと、このバイト列が何かを示す。",
    ],
    src: `${P}/wrap.rs:47`,
  },
  "label.key-commit": {
    kind: "label",
    known: ALL,
    says: [
      "Label for key commitments, so a commitment cannot be confused with any other hash.",
      "钥匙承诺用的标签，所以承诺不会和别的哈希混淆。",
      "金鑰承諾用的標籤，所以承諾不會和別的雜湊混淆。",
      "鍵のコミットメント用のラベル。ほかのハッシュと取り違えられない。",
    ],
    src: `${P}/chain.rs:130-131`,
  },
  "label.rh-v1": {
    kind: "label",
    known: ALL,
    says: ["Starts the channel's prologue: protocol version 1.", "通道 prologue 的开头：协议第 1 版。", "通道 prologue 的開頭：協定第 1 版。", "チャネルの prologue の先頭：プロトコルの第 1 版。"],
    src: `${P}/noise.rs:60-61`,
  },
  "label.direct": {
    kind: "label",
    known: ALL,
    says: [
      "Starts the direct path's prologue, so it can never equal a relayed channel's.",
      "直连路径 prologue 的开头，所以它永远不会等于经中继的通道的 prologue。",
      "直連路徑 prologue 的開頭，所以它永遠不會等於經中繼的通道的 prologue。",
      "直結経路の prologue の先頭。中継経由のチャネルの prologue と一致することはない。",
    ],
    src: `${P}/path.rs:109-110`,
  },
  "label.stream-key": {
    kind: "label",
    known: ALL,
    says: [
      "HKDF salt for stream keys; stream names come from the same content key under another salt.",
      "流钥用的 HKDF salt；流的名字也出自同一把内容密钥，但用另一个 salt。",
      "串流金鑰用的 HKDF salt；串流的名字也出自同一把內容金鑰，但用另一個 salt。",
      "ストリーム鍵用の HKDF の salt。ストリームの名前も同じコンテンツ鍵から、別の salt で作る。",
    ],
    src: `${P}/record.rs:112-113`,
  },
  "label.record": {
    kind: "label",
    known: ALL,
    says: ["Starts each record's AAD.", "每条记录 AAD 的开头。", "每筆紀錄 AAD 的開頭。", "各レコードの AAD の先頭。"],
    src: `${P}/record.rs:114`,
  },
  "label.record-link": {
    kind: "label",
    known: ALL,
    says: ["Label for the running hash over a stream's records.", "流中记录的累积哈希用的标签。", "串流中紀錄的累積雜湊用的標籤。", "ストリームのレコードをつなぐ累積ハッシュ用のラベル。"],
    src: `${P}/record.rs:115`,
  },
  "label.stream-head": {
    kind: "label",
    known: ALL,
    says: [
      "Label for stream heads, signed by the device that runs AI.",
      "流头用的标签，由可运行 AI 的设备签名。",
      "串流頭用的標籤，由可執行 AI 的裝置簽章。",
      "ストリームヘッド用のラベル。AI を実行できるデバイスが署名する。",
    ],
    src: `${P}/sig.rs:12-35`,
  },
  "label.page": {
    kind: "label",
    known: ALL,
    says: [
      "Starts each snapshot page's AAD, so a page can never be taken for a record.",
      "每个快照页 AAD 的开头，所以页永远不会被当成记录。",
      "每個快照頁 AAD 的開頭，所以頁永遠不會被當成紀錄。",
      "スナップショットページの AAD の先頭。ページがレコードと取り違えられることはない。",
    ],
    src: `${P}/stream.rs:102`,
  },
  "label.stream-manifest": {
    kind: "label",
    known: ALL,
    says: [
      "Label for manifests: the signed list of a stream's snapshot pages.",
      "清单用的标签：一个流的快照页的签名列表。",
      "清單用的標籤：一個串流的快照頁的簽章列表。",
      "マニフェスト用のラベル：ストリームのスナップショットページの署名付き一覧。",
    ],
    src: `${P}/sig.rs:13-36`,
  },
  "label.stream-draft": {
    kind: "label",
    known: ALL,
    says: [
      "Label for drafts: a turn's live progress (today, how far a think has got), sent to live readers, never stored, signed by the device that runs AI.",
      "草稿用的标签：一轮对话的实时进展（目前是思考进行到哪了），只发给在线的读者、从不存储，由可运行 AI 的设备签名。",
      "草稿用的標籤：一輪對話的即時進展（目前是思考進行到哪了），只發給線上的讀者、從不儲存，由可執行 AI 的裝置簽章。",
      "下書き用のラベル：ターンのその場の進み具合（いまは思考がどこまで進んだか）。その場で読んでいる相手にだけ送り、保存はせず、AI を実行できるデバイスが署名する。",
    ],
    src: "server-rs/src/inject/drafts.rs:1-30",
  },
  "label.hub-login": {
    kind: "label",
    known: ALL,
    says: [
      "Label for signing in to Droppen Hub or a relay: its challenge is signed inside a structure of our own, never bare.",
      "登录 Droppen Hub 或中继用的标签：对方的挑战放在我们自己的结构里签，从不单独签。",
      "登入 Droppen Hub 或中繼用的標籤：對方的挑戰放在我們自己的結構裡簽，從不單獨簽。",
      "Droppen Hub や中継へのサインイン用のラベル：相手のチャレンジは自前の構造に入れて署名し、それ単体には署名しない。",
    ],
    src: `${P}/sig.rs:106-147`,
  },
  "label.push": {
    kind: "label",
    known: ALL,
    says: [
      "Reserved for Web Push payloads; nothing is signed under it today.",
      "留给 Web Push 内容用；目前没有任何东西用它签名。",
      "留給 Web Push 內容用；目前沒有任何東西用它簽章。",
      "Web Push の内容用に予約済み。いまはこのラベルで署名するものはない。",
    ],
    src: `${P}/sig.rs:15-38`,
  },
  "label.recovery-proof": {
    kind: "label",
    known: ALL,
    says: [
      "Label for the recovery key's signature over the account and its account key: only the holder of the words can put that key into a chain.",
      "恢复钥对账户和账户钥的签名用的标签：只有握着恢复词的人，才能把这把钥放进链里。",
      "復原金鑰對帳戶和帳戶金鑰的簽章用的標籤：只有握著復原詞的人，才能把這把金鑰放進鏈裡。",
      "リカバリー鍵がアカウントとアカウント鍵に付ける署名のラベル。リカバリーワードを持つ人しか、その鍵をチェーンに入れられない。",
    ],
    src: `${P}/chain.rs:164-204`,
  },
  "label.passkey-proof": {
    kind: "label",
    known: ALL,
    says: [
      "Label for a passkey slot key's proof over the account, its account key, the slot, the credential and the salt.",
      "通行密钥槽位钥的证明用的标签，覆盖账户、账户钥、槽位、凭据和 salt。",
      "通行金鑰槽位金鑰的證明用的標籤，涵蓋帳戶、帳戶金鑰、槽位、憑證和 salt。",
      "パスキースロットの鍵による証明のラベル。アカウント、アカウント鍵、スロット、認証情報、salt を対象にする。",
    ],
    src: `${P}/chain.rs:206-255`,
  },
  "label.relay-ticket": {
    kind: "label",
    known: ALL,
    says: [
      "Label for Droppen Hub's 15-minute ticket that lets one device use one relay.",
      "Droppen Hub 发的 15 分钟票据用的标签，让一台设备使用一个中继。",
      "Droppen Hub 發的 15 分鐘票據用的標籤，讓一台裝置使用一個中繼。",
      "Droppen Hub が発行する 15 分間のチケットのラベル。1 台のデバイスが 1 つの中継を使えるようにする。",
    ],
    src: `${P}/ticket.rs:1-27`,
  },
  "label.relay-origin": {
    kind: "label",
    known: ALL,
    says: [
      "Label for a relay's proof that it answers at its own address.",
      "中继证明自己在这个地址应答用的标签。",
      "中繼證明自己在這個位址回應用的標籤。",
      "中継が自分のアドレスで応答していることの証明に使うラベル。",
    ],
    src: `${P}/ticket.rs:261-276`,
  },
  "label.host-rotate": {
    kind: "label",
    known: ALL,
    says: [
      "Label for the renewal proof, signed by both the old and the new identity key.",
      "更换证明用的标签，新旧两把身份密钥都要签。",
      "更換證明用的標籤，新舊兩把身分金鑰都要簽。",
      "更新の証明用のラベル。古い ID 鍵と新しい ID 鍵の両方が署名する。",
    ],
    src: `${P}/sig.rs:44-46`,
  },
  "label.rotate-confirm": {
    kind: "label",
    known: ALL,
    says: [
      "Label for the MAC that proves the new X25519 key is held.",
      "证明握有新 X25519 钥的那个 MAC 用的标签。",
      "證明握有新 X25519 金鑰的那個 MAC 用的標籤。",
      "新しい X25519 鍵を持っていることを示す MAC 用のラベル。",
    ],
    src: `${P}/chain.rs:257-259`,
  },
  "label.rotate-ack": {
    kind: "label",
    known: ALL,
    says: [
      "Label for the new key's confirmation of a staged renewal at sign-in.",
      "登录时新钥确认暂存的更换用的标签。",
      "登入時新金鑰確認暫存的更換用的標籤。",
      "サインイン時に新しい鍵が保留中の更新を確認するためのラベル。",
    ],
    src: `${P}/sig.rs:47-49`,
  },
  "label.rotate-sas": {
    kind: "label",
    known: ALL,
    says: [
      "Label for the code shown on the device that runs AI.",
      "可运行 AI 的设备上显示的代码用的标签。",
      "可執行 AI 的裝置上顯示的代碼用的標籤。",
      "AI を実行できるデバイスに表示するコード用のラベル。",
    ],
    src: `${P}/chain.rs:260-261`,
  },
  "label.rotate-dispute": {
    kind: "label",
    known: ALL,
    says: ["Label for a dispute signed with the retired key.", "用退役钥签的争议声明用的标签。", "用退役金鑰簽的爭議聲明用的標籤。", "退役した鍵で署名する異議申し立て用のラベル。"],
    src: `${P}/sig.rs:50-52`,
  },

  // ---- 2.1 chain entries
  "chain.entry": {
    kind: "value",
    known: STORED,
    says: [
      "One chain entry as stored and sent: fixed binary fields, then the signature. Never JSON, so there is exactly one byte string to sign and hash.",
      "一条链条目，存的、传的都是它：固定的二进制字段，后面跟签名。绝不是 JSON，所以要签、要算哈希的字节只有一种。",
      "一筆鏈項目，存的、傳的都是它：固定的二進位欄位，後面跟簽章。絕不是 JSON，所以要簽、要算雜湊的位元組只有一種。",
      "保存も送信もこの形のチェーンのエントリ：固定のバイナリフィールドと、その後の署名。JSON ではないので、署名しハッシュするバイト列は 1 通りしかない。",
    ],
    src: `${P}/chain.rs:1225-1230`,
  },
  "chain.version": {
    kind: "value",
    known: STORED,
    says: [
      "Encoding version: 2, or 3 for chain v3. It is signed, so an entry cannot be reread under the other version's rules.",
      "编码版本：2，或链 v3 的 3。它在签名范围内，所以条目不能被换成另一个版本的规则来读。",
      "編碼版本：2，或鏈 v3 的 3。它在簽章範圍內，所以項目不能被換成另一個版本的規則來讀。",
      "エンコードの版：2、またはチェーン v3 の 3。署名の対象なので、もう一方の版の規則で読み直すことはできない。",
    ],
    src: `${P}/chain.rs:113-117`,
  },
  "chain.seq": {
    kind: "value",
    known: STORED,
    says: [
      "Position in the chain: 0 for the genesis, then one more each time. Verifiers refuse a gap, and a chain that goes back.",
      "在链上的位置：创世是 0，之后每条加一。验证者不接受缺号，也不接受往回走的链。",
      "在鏈上的位置：創世是 0，之後每筆加一。驗證者不接受缺號，也不接受往回走的鏈。",
      "チェーン上の位置：ジェネシスが 0、以後 1 ずつ増える。検証者は欠番も、後戻りするチェーンも拒む。",
    ],
    src: `${P}/chain.rs:89-93`,
  },
  "chain.prev": {
    kind: "value",
    known: STORED,
    rel: ["chain.hash"],
    says: [
      "The previous entry's hash (next.prev below). It links each entry to the one before, so none can be removed, swapped or slipped in.",
      "前一条的哈希（下面的 next.prev）。它把每条和前一条连起来，所以哪条都删不掉、换不了、插不进。",
      "前一筆的雜湊（下面的 next.prev）。它把每筆和前一筆連起來，所以哪筆都刪不掉、換不了、插不進。",
      "1 つ前のエントリのハッシュ（下の next.prev）。各エントリを前のものにつなぐので、削除も差し替えも割り込みもできない。",
    ],
    src: `${P}/chain.rs:45-51`,
  },
  "chain.time": {
    kind: "value",
    known: STORED,
    says: [
      "When the signer wrote it, in milliseconds. Signed with the rest, so nobody can change it afterwards.",
      "签名者写下它的时间，单位毫秒。和其余部分一起签名，事后谁也改不了。",
      "簽章者寫下它的時間，單位毫秒。和其餘部分一起簽章，事後誰也改不了。",
      "署名者が書いた時刻（ミリ秒）。ほかと一緒に署名されるので、後から誰も変えられない。",
    ],
    src: `${P}/chain.rs:1137-1143`,
  },
  "chain.signer": {
    kind: "value",
    known: STORED,
    says: [
      "Who signs: the account key (the genesis only), a device by its id, the recovery key, or a passkey slot. Verifiers check that it may sign this kind.",
      "谁签的：账户钥（只签创世）、某台设备（按 id）、恢复钥，或某个通行密钥槽位。验证者会核对它能不能签这一种。",
      "誰簽的：帳戶金鑰（只簽創世）、某台裝置（按 id）、復原金鑰，或某個通行金鑰槽位。驗證者會核對它能不能簽這一種。",
      "署名者：アカウント鍵（ジェネシスのみ）、id で示すデバイス、リカバリー鍵、パスキースロットのいずれか。検証者はこの種類に署名してよいかを確かめる。",
    ],
    src: `${P}/chain.rs:775-785`,
  },
  "chain.kind": {
    kind: "value",
    known: STORED,
    says: [
      "Which action, in one byte: 12 adds a device, 15 removes one, 10 commits key epochs… An unknown kind does not decode.",
      "哪种动作，一个字节：12 是加设备，15 是移除，10 是提交钥匙轮次……不认识的种类一律解不出来。",
      "哪種動作，一個位元組：12 是加裝置，15 是移除，10 是提交金鑰輪次……不認識的種類一律解不出來。",
      "どの操作かを 1 バイトで示す：12 はデバイス追加、15 は削除、10 は鍵の世代のコミット…。知らない種類はデコードされない。",
    ],
    src: `${P}/chain.rs:731-757`,
  },
  "chain.action": {
    kind: "value",
    known: STORED,
    says: [
      "The action's own fields: a device and its public keys, an id, key commitments…",
      "动作自己的字段：一台设备和它的公钥、一个 id、钥匙承诺……",
      "動作自己的欄位：一台裝置和它的公鑰、一個 id、金鑰承諾……",
      "操作そのもののフィールド：デバイスとその公開鍵、id、鍵のコミットメント…。",
    ],
    src: `${P}/chain.rs:1156-1216`,
  },
  "chain.signature": {
    kind: "value",
    known: STORED,
    says: [
      "The entry's signature (line 2). Every device checks it with the signer's key from its own copy of the chain.",
      "条目的签名（第 2 行）。每台设备都用自己那份链上签名者的钥来验。",
      "項目的簽章（第 2 行）。每台裝置都用自己那份鏈上簽章者的金鑰來驗。",
      "エントリの署名（2 行目）。どのデバイスも、自分のチェーンにある署名者の鍵で検証する。",
    ],
    src: `${P}/chain.rs:1219-1222`,
  },
  "chain.signer-key": {
    kind: "secret",
    known: ME,
    says: [
      "The signer's private key: here your full device's own. The account key signs only the genesis, and is then thrown away.",
      "签名者的私钥：这里是你这台完整设备自己的。账户钥只签创世，签完就丢掉。",
      "簽章者的私鑰：這裡是你這台完整裝置自己的。帳戶金鑰只簽創世，簽完就丟掉。",
      "署名者の秘密鍵：ここではあなたのフルデバイス自身の鍵。アカウント鍵はジェネシスにだけ署名し、その後捨てられる。",
    ],
    src: `${P}/chain.rs:53-62`,
  },
  "chain.hash": {
    kind: "value",
    known: STORED,
    says: [
      "This entry's hash, which the next entry carries as prev. Verifiers keep the newest they have seen, so a fork or a rollback shows.",
      "这条的哈希，下一条把它记作 prev。验证者记着见过的最新一个，所以分叉或回滚都会露馅。",
      "這筆的雜湊，下一筆把它記作 prev。驗證者記著見過的最新一個，所以分叉或回滾都會露餡。",
      "このエントリのハッシュ。次のエントリが prev として持つ。検証者は見た中で最新のものを覚えているので、フォークやロールバックは露見する。",
    ],
    src: `${P}/chain.rs:1331-1337`,
  },

  // ---- 2.6 attestations
  "attest.sig": {
    kind: "value",
    known: STORED,
    says: [
      "A full device's signed word: “at this time, the chain I see ends here.” Without a recent one, a device that runs AI opens no channel.",
      "完整设备签名的一句话：“此刻我看到的链到这里为止。”没有近期的这句话，可运行 AI 的设备不开任何通道。",
      "完整裝置簽章的一句話：「此刻我看到的鏈到這裡為止。」沒有近期的這句話，可執行 AI 的裝置不開任何通道。",
      "フルデバイスが署名した一言：「この時点で、私の見ているチェーンはここで終わる」。最近のものがなければ、AI を実行できるデバイスはチャネルを開かない。",
    ],
    src: `${P}/chain.rs:1623-1657`,
  },
  "admin.key": {
    kind: "secret",
    known: ME,
    says: [
      "Your full device's private Ed25519 key. Only a current, active full device may sign this.",
      "你这台完整设备的 Ed25519 私钥。只有当前有效的完整设备才能签这个。",
      "你這台完整裝置的 Ed25519 私鑰。只有目前有效的完整裝置才能簽這個。",
      "あなたのフルデバイスの Ed25519 秘密鍵。これに署名できるのは、現役で有効なフルデバイスだけ。",
    ],
    src: `${P}/chain.rs:3068-3076`,
  },
  "attest.seq": {
    kind: "value",
    known: STORED,
    says: [
      "The sequence number of the newest entry this device sees. It counts only if it equals the verifier's own head.",
      "这台设备看到的最新条目的序号。它要等于验证者自己的链头才算数。",
      "這台裝置看到的最新項目的序號。它要等於驗證者自己的鏈頭才算數。",
      "このデバイスが見ている最新エントリの連番。検証者自身の先頭と一致したときだけ有効。",
    ],
    src: `${P}/chain.rs:3116-3118`,
  },
  "attest.head": {
    kind: "value",
    known: STORED,
    rel: ["chain.hash"],
    says: [
      "That entry's hash (next.prev in 2.1): the attestation names one exact chain, not just its length.",
      "那条的哈希（2.1 里的 next.prev）：作证指的是确切的一条链，不只是长度。",
      "那筆的雜湊（2.1 裡的 next.prev）：作證指的是確切的一條鏈，不只是長度。",
      "そのエントリのハッシュ（2.1 の next.prev）。証明が指すのは長さだけでなく、ただ 1 つのチェーン。",
    ],
    src: `${P}/chain.rs:1636-1644`,
  },
  "attest.time": {
    kind: "value",
    known: STORED,
    says: [
      "When it was signed: its age is the freshness checked below. A time more than 5 minutes ahead is not counted.",
      "签名的时间：它有多旧，就是下面要查的新鲜度。超前 5 分钟以上的不算。",
      "簽章的時間：它有多舊，就是下面要查的新鮮度。超前 5 分鐘以上的不算。",
      "署名した時刻。その古さが下で調べる鮮度になる。5 分以上先の時刻は数えない。",
    ],
    src: `${P}/chain.rs:3116-3127`,
  },
  "const.control": {
    kind: "label",
    known: ALL,
    says: [
      "How recent an attestation of the current head must be for a device that runs AI to accept control at all.",
      "当前链头的作证要多新，可运行 AI 的设备才肯被控制。",
      "目前鏈頭的作證要多新，可執行 AI 的裝置才肯被控制。",
      "AI を実行できるデバイスが操作を受け付けるのに必要な、現在の先頭の証明の新しさ。",
    ],
    src: `${P}/chain.rs:138-140`,
  },
  "const.manage": {
    kind: "label",
    known: ALL,
    says: [
      "The stricter limit for requests that change its keys or what it is: a content key, its name, re-encryption.",
      "更严的期限，用于会改动它的钥或它本身的请求：内容密钥、名字、重新加密。",
      "更嚴的期限，用於會改動它的金鑰或它本身的請求：內容金鑰、名字、重新加密。",
      "鍵やそのデバイス自体を変える要求（コンテンツ鍵、名前、再暗号化）に使う、より厳しい期限。",
    ],
    src: `${P}/chain.rs:141-143`,
  },
  "const.attest-every": {
    kind: "label",
    known: ALL,
    says: [
      "How often an open page attests the head again even when nothing changed; it also attests on connecting and after each change.",
      "页面开着时，即使什么都没变也每隔这么久重新作证一次；连上时和每次变化后也会作证。",
      "頁面開著時，即使什麼都沒變也每隔這麼久重新作證一次；連上時和每次變化後也會作證。",
      "ページを開いている間、何も変わらなくてもこの間隔で先頭を証明し直す。接続時と変更のたびにも証明する。",
    ],
    src: "web/src/lib/hub/follow.ts:8-26",
  },

  // ---- 3.1 delivering a key
  "seal.kek": {
    kind: "derived",
    known: ENDS,
    says: [
      "Key-encryption key for this one delivery. Only the sender and the recipient can compute it.",
      "这一次投递专用的密钥加密钥。只有发送方和接收方算得出来。",
      "這一次投遞專用的金鑰加密金鑰。只有傳送方和接收方算得出來。",
      "この 1 回の配送専用の鍵暗号化鍵。送信者と受信者にしか計算できない。",
    ],
    src: `${P}/wrap.rs:180-185`,
  },
  "hkdf.salt": {
    kind: "label",
    known: ALL,
    says: [
      "HKDF's salt: a fixed public label for each use, so the same secret gives unrelated keys in different roles.",
      "HKDF 的 salt：每种用途一个固定的公开标签，所以同一份秘密在不同用途下得出毫不相干的钥。",
      "HKDF 的 salt：每種用途一個固定的公開標籤，所以同一份秘密在不同用途下得出毫不相干的金鑰。",
      "HKDF の salt：用途ごとに決まった公開のラベル。同じ秘密でも用途が違えば無関係な鍵になる。",
    ],
    src: `${P}/wrap.rs:157-163`,
  },
  "seal.e-priv": {
    kind: "secret",
    known: ME,
    rel: ["seal.e-pub"],
    says: [
      "A one-off X25519 private key the sender makes for this delivery and then forgets.",
      "发送方为这一次投递临时生成、用完就忘的 X25519 私钥。",
      "傳送方為這一次投遞臨時產生、用完就忘的 X25519 私鑰。",
      "送信者がこの配送のためだけに作り、使ったら忘れる使い捨ての X25519 秘密鍵。",
    ],
    src: `${P}/wrap.rs:198-211`,
  },
  "seal.R": {
    kind: "public",
    known: STORED,
    says: [
      "The recipient's X25519 public key, from the chain. Only its private key can open the delivery.",
      "接收方的 X25519 公钥，取自链上。只有它对应的私钥打得开这份投递。",
      "接收方的 X25519 公鑰，取自鏈上。只有它對應的私鑰打得開這份投遞。",
      "受信者の X25519 公開鍵（チェーンから）。この配送を開けるのは、対応する秘密鍵だけ。",
    ],
    src: `${P}/wrap.rs:187-196`,
  },
  "seal.S-priv": {
    kind: "secret",
    known: ME,
    rel: ["seal.S-pub"],
    says: [
      "The sender's own static X25519 private key: only that device could have sealed this, so the Hub cannot slip in a key of its own.",
      "发送方自己的静态 X25519 私钥：只有那台设备封得出这一份，所以 Hub 塞不进自己挑的钥。",
      "傳送方自己的靜態 X25519 私鑰：只有那台裝置封得出這一份，所以 Hub 塞不進自己挑的金鑰。",
      "送信者自身の静的な X25519 秘密鍵。これを封印できたのはそのデバイスだけなので、Hub が自分で選んだ鍵を紛れ込ませることはできない。",
    ],
    src: `${P}/wrap.rs:4-12`,
  },
  "hkdf.info": {
    kind: "value",
    known: STORED,
    says: [
      "HKDF's info: public context bound into the key, so the key is good for this exact context only.",
      "HKDF 的 info：绑进钥里的公开上下文，所以这把钥只对这一个上下文有效。",
      "HKDF 的 info：綁進金鑰裡的公開上下文，所以這把金鑰只對這一個上下文有效。",
      "HKDF の info：鍵に結び付ける公開のコンテキスト。鍵はこのコンテキストでしか通用しない。",
    ],
    src: `${P}/wrap.rs:180-185`,
  },
  "seal.e-pub": {
    kind: "public",
    known: STORED,
    rel: ["seal.e-priv"],
    says: [
      "The one-off key's public half. It travels with the sealed key, so the recipient can compute the same secret from its side.",
      "临时钥的公钥那一半。它跟着封好的钥一起走，接收方用它从自己这边算出同一份秘密。",
      "臨時金鑰的公鑰那一半。它跟著封好的金鑰一起走，接收方用它從自己這邊算出同一份秘密。",
      "使い捨て鍵の公開鍵の側。封印した鍵と一緒に届くので、受信者は自分の側から同じ秘密を計算できる。",
    ],
    src: `${P}/wrap.rs:109-115`,
  },
  "seal.S-pub": {
    kind: "public",
    known: STORED,
    rel: ["seal.S-priv"],
    says: [
      "The sender's public key, from the chain. With all three public keys in it, the key holds only between these two devices.",
      "发送方的公钥，取自链上。三把公钥都放进去，这把钥就只在这两台设备之间成立。",
      "傳送方的公鑰，取自鏈上。三把公鑰都放進去，這把金鑰就只在這兩台裝置之間成立。",
      "送信者の公開鍵（チェーンから）。3 つの公開鍵がすべて入るので、この鍵はこの 2 台の間でしか成り立たない。",
    ],
    src: `${P}/wrap.rs:221-231`,
  },
  "seal.ctx": {
    kind: "value",
    known: STORED,
    says: [
      "The delivery's context. It is in the key and is the AAD, so a sealed key cannot pass for another device's, another epoch's, or a user key.",
      "这份投递的上下文。它既在钥里，又是 AAD，所以封好的钥冒充不了别的设备的、别的轮次的，也冒充不了用户钥。",
      "這份投遞的上下文。它既在金鑰裡，又是 AAD，所以封好的金鑰冒充不了別的裝置的、別的輪次的，也冒充不了使用者金鑰。",
      "配送のコンテキスト。鍵に入り、AAD でもあるので、封印した鍵を別のデバイス宛て、別の世代、ユーザー鍵として通すことはできない。",
    ],
    src: `${P}/wrap.rs:16-19`,
  },
  "seal.purpose": {
    kind: "value",
    known: STORED,
    says: [
      "What is sealed: 1 a user key, 2 a content key for its own device that runs AI, 3 a content key for another device, which adds the host's id after the epoch.",
      "封的是什么：1 是用户钥，2 是给可运行 AI 的设备它自己的内容密钥，3 是给别的设备的内容密钥，此时轮次后面再加上那台设备的 id。",
      "封的是什麼：1 是使用者金鑰，2 是給可執行 AI 的裝置它自己的內容金鑰，3 是給別的裝置的內容金鑰，此時輪次後面再加上那台裝置的 id。",
      "封印したもの：1 はユーザー鍵、2 は AI を実行できるデバイス自身のコンテンツ鍵、3 は別のデバイスに渡すコンテンツ鍵で、このときは世代の後にそのデバイスの id が続く。",
    ],
    src: `${P}/wrap.rs:233-264`,
  },
  "seal.sender": {
    kind: "value",
    known: ALL,
    says: [
      "The sender's id; the seal is checked with its key from the chain. Keys come only from a current full device, except a recovery package, which the chain's commitment vouches for.",
      "发送方的 id；用链上它的钥核对封装。钥只收当前完整设备发来的；恢复包例外，由链上的承诺为它担保。",
      "傳送方的 id；用鏈上它的金鑰核對封裝。金鑰只收目前完整裝置發來的；復原包例外，由鏈上的承諾為它擔保。",
      "送信者の id。封印はチェーンにあるその鍵で確かめる。鍵は現役のフルデバイスからしか受け取らないが、リカバリーパッケージは例外で、チェーンのコミットメントがその正しさを保証する。",
    ],
    src: `${P}/wrap.rs:333-430`,
  },
  "seal.recipient": {
    kind: "value",
    known: ALL,
    says: [
      "The id it is for (“recovery” for the recovery words). A device refuses a key addressed to anyone else.",
      "收件方的 id（恢复词的那份是 “recovery”）。设备拒收写给别人的钥。",
      "收件方的 id（復原詞的那份是「recovery」）。裝置拒收寫給別人的金鑰。",
      "宛先の id（リカバリーワード宛ては “recovery”）。デバイスはほかの宛先の鍵を受け取らない。",
    ],
    src: `${P}/wrap.rs:333-349`,
  },
  "seal.epoch": {
    kind: "value",
    known: STORED,
    says: [
      "Which epoch of the key. It must be one the chain committed, and epochs move up one at a time.",
      "钥的哪一轮。必须是链上提交过的那一轮，而轮次每次只能加一。",
      "金鑰的哪一輪。必須是鏈上提交過的那一輪，而輪次每次只能加一。",
      "鍵のどの世代か。チェーンがコミットした世代でなければならず、世代は 1 つずつしか進まない。",
    ],
    src: `${P}/chain.rs:478-488`,
  },
  "seal.aad": {
    kind: "value",
    known: STORED,
    rel: ["seal.ctx"],
    says: [
      "AES-GCM's additional data: not encrypted, but it must match to decrypt. Here it is ctx.",
      "AES-GCM 的附加数据：不加密，但必须对得上才能解密。这里就是 ctx。",
      "AES-GCM 的附加資料：不加密，但必須對得上才能解密。這裡就是 ctx。",
      "AES-GCM の追加データ：暗号化はしないが、一致しなければ復号できない。ここでは ctx。",
    ],
    src: `${P}/wrap.rs:212-218`,
  },
  "key.commit": {
    kind: "value",
    known: STORED,
    says: [
      "The key's hash, put on the chain (KeyEpochs) before the key is sent. A key that does not match it is refused.",
      "钥的哈希，在钥发出之前先写上链（KeyEpochs）。对不上的钥一律拒收。",
      "金鑰的雜湊，在金鑰發出之前先寫上鏈（KeyEpochs）。對不上的金鑰一律拒收。",
      "鍵のハッシュ。鍵を送る前にチェーン（KeyEpochs）に載せる。一致しない鍵は拒否される。",
    ],
    src: `${P}/chain.rs:470-511`,
  },
  "key.kind": {
    kind: "value",
    known: STORED,
    says: ["1 for a user key, 2 for a content key.", "1 是用户钥，2 是内容密钥。", "1 是使用者金鑰，2 是內容金鑰。", "1 はユーザー鍵、2 はコンテンツ鍵。"],
    src: `${P}/chain.rs:503-511`,
  },
  "key.id": {
    kind: "value",
    known: ALL,
    says: [
      "Whose key: the id of the device that runs AI for a content key, empty for the user key.",
      "谁的钥：内容密钥填可运行 AI 的设备的 id，用户钥留空。",
      "誰的金鑰：內容金鑰填可執行 AI 的裝置的 id，使用者金鑰留空。",
      "誰の鍵か：コンテンツ鍵なら AI を実行できるデバイスの id、ユーザー鍵なら空。",
    ],
    src: `${P}/chain.rs:503-511`,
  },
  "key.key": {
    kind: "secret",
    known: ENDS,
    says: [
      "The key being delivered: a user key or a content key. Its commitment reveals nothing about it.",
      "要送的那把钥：用户钥或内容密钥。它的承诺透露不出它的任何内容。",
      "要送的那把金鑰：使用者金鑰或內容金鑰。它的承諾透露不出它的任何內容。",
      "届ける鍵そのもの：ユーザー鍵かコンテンツ鍵。コミットメントからは何もわからない。",
    ],
    src: `${P}/chain.rs:490-501`,
  },

  // ---- 4.1 and 4.4 channels
  "noise.Noise": {
    kind: "label",
    known: ALL,
    says: [
      "The Noise Protocol Framework: a published, analysed recipe for encrypted channels.",
      "Noise 协议框架：公开发表、经过分析的加密通道配方。",
      "Noise 協定框架：公開發表、經過分析的加密通道配方。",
      "Noise Protocol Framework：公開され分析されている、暗号化チャネルの組み立て方。",
    ],
    src: `${P}/noise.rs:4-10`,
  },
  "noise.IK": {
    kind: "label",
    known: ALL,
    says: [
      "The handshake pattern: your device already Knows the other side's static key (from the chain) and sends its own Immediately, encrypted.",
      "握手模式：你的设备事先就知道（K）对方的静态钥（取自链上），自己的静态钥立刻（I）加密发出。",
      "交握模式：你的裝置事先就知道（K）對方的靜態金鑰（取自鏈上），自己的靜態金鑰立刻（I）加密送出。",
      "ハンドシェイクのパターン：あなたのデバイスは相手の静的鍵を最初から知っていて（K、チェーンから）、自分の静的鍵はすぐに（I）暗号化して送る。",
    ],
    src: `${P}/noise.rs:12-16`,
  },
  "noise.prologue": {
    kind: "value",
    known: ALL,
    says: [
      "Bytes both ends mix into the handshake without sending them. If they disagree, the handshake fails: no relay or Hub can splice two sessions.",
      "双方各自混进握手、但不发送的字节。只要两边不一致，握手就失败：中继和 Hub 都拼接不了两个会话。",
      "雙方各自混進交握、但不傳送的位元組。只要兩邊不一致，交握就失敗：中繼和 Hub 都拼接不了兩個工作階段。",
      "両端がハンドシェイクに混ぜるが、送りはしないバイト列。食い違えばハンドシェイクは失敗するので、中継も Hub も 2 つのセッションをつなぎ替えられない。",
    ],
    src: `${P}/noise.rs:112-135`,
  },
  "direct.prologue": {
    kind: "value",
    known: ENDS,
    says: [
      "The direct path's prologue: it also binds the relayed channel, a fresh nonce and both DTLS certificates, so nobody can sit in between.",
      "直连路径的 prologue：还绑定了经中继的那条通道、一个新的 nonce 和双方的 DTLS 证书，所以谁也插不到中间。",
      "直連路徑的 prologue：還綁定了經中繼的那條通道、一個新的 nonce 和雙方的 DTLS 憑證，所以誰也插不到中間。",
      "直結経路の prologue：中継経由のチャネル、新しい nonce、双方の DTLS 証明書も結び付けるので、誰も間に入れない。",
    ],
    src: `${P}/path.rs:488-521`,
  },
  "direct.channel-hash": {
    kind: "value",
    known: STORED,
    says: [
      "The relayed channel's Noise handshake hash: this direct path belongs to that channel and no other.",
      "经中继的那条通道的 Noise 握手哈希：这条直连只属于那条通道。",
      "經中繼的那條通道的 Noise 交握雜湊：這條直連只屬於那條通道。",
      "中継経由のチャネルの Noise ハンドシェイクのハッシュ。この直結はそのチャネルだけのもの。",
    ],
    src: `${P}/path.rs:493-495`,
  },
  "direct.nonce": {
    kind: "value",
    known: ENDS,
    says: [
      "A fresh random value from the device that runs AI for this attempt, so an old direct handshake cannot be replayed.",
      "可运行 AI 的设备为这一次尝试给的新随机值，所以旧的直连握手没法重放。",
      "可執行 AI 的裝置為這一次嘗試給的新隨機值，所以舊的直連交握沒辦法重放。",
      "AI を実行できるデバイスがこの試行のために出す新しい乱数。古い直結のハンドシェイクは再利用できない。",
    ],
    src: `${P}/path.rs:496-497`,
  },
  "direct.device-fp": {
    kind: "value",
    known: ENDS,
    says: [
      "Your device's DTLS certificate fingerprint, stated inside the encrypted channel.",
      "你的设备的 DTLS 证书指纹，在加密通道里说明。",
      "你的裝置的 DTLS 憑證指紋，在加密通道裡說明。",
      "あなたのデバイスの DTLS 証明書のフィンガープリント。暗号化されたチャネルの中で伝える。",
    ],
    src: `${P}/path.rs:12-18`,
  },
  "direct.host-fp": {
    kind: "value",
    known: ENDS,
    says: [
      "The DTLS certificate fingerprint of the device that runs AI. Swap a certificate and the handshake fails.",
      "可运行 AI 的设备的 DTLS 证书指纹。证书一被调换，握手就失败。",
      "可執行 AI 的裝置的 DTLS 憑證指紋。憑證一被調換，交握就失敗。",
      "AI を実行できるデバイスの DTLS 証明書のフィンガープリント。証明書をすり替えればハンドシェイクは失敗する。",
    ],
    src: `${P}/path.rs:498-501`,
  },

  // ---- 5.1–5.3 streams
  "stream.SK": {
    kind: "derived",
    known: ENDS,
    says: [
      "Stream key: encrypts one stream's records and pages. Derived from the content key when needed; never stored or sent.",
      "流钥：加密一个流的记录和页。需要时从内容密钥派生，从不存储、从不发送。",
      "串流金鑰：加密一個串流的紀錄和頁。需要時從內容金鑰衍生，從不儲存、從不傳送。",
      "ストリーム鍵：1 つのストリームのレコードとページを暗号化する。必要なときにコンテンツ鍵から導出し、保存も送信もしない。",
    ],
    src: `${P}/record.rs:174-181`,
  },
  "stream.MK": {
    kind: "secret",
    known: ENDS,
    says: [
      "The content key of the device that runs AI, epoch e. It is only ever an input to HKDF, under a different salt for each use.",
      "可运行 AI 的设备第 e 轮的内容密钥。它只当 HKDF 的输入，每种用途换一个 salt。",
      "可執行 AI 的裝置第 e 輪的內容金鑰。它只當 HKDF 的輸入，每種用途換一個 salt。",
      "AI を実行できるデバイスの第 e 世代のコンテンツ鍵。HKDF の入力にしか使わず、用途ごとに salt を変える。",
    ],
    src: `${P}/record.rs:12-14`,
  },
  "stream.id": {
    kind: "value",
    known: STORED,
    says: [
      "The stream's name: an HMAC under a key from the content key, so the Hub cannot tell which session it is.",
      "流的名字：用一把出自内容密钥的钥算的 HMAC，所以 Hub 看不出这是哪个会话。",
      "串流的名字：用一把出自內容金鑰的金鑰算的 HMAC，所以 Hub 看不出這是哪個工作階段。",
      "ストリームの名前：コンテンツ鍵から作った鍵による HMAC。Hub にはどのセッションかわからない。",
    ],
    src: `${P}/record.rs:161-172`,
  },
  "record.aad": {
    kind: "value",
    known: STORED,
    says: [
      "Bound into each record's encryption: a record moved to another stream, position or epoch no longer decrypts.",
      "绑进每条记录的加密：记录被挪到别的流、别的位置或别的轮次，就解不开了。",
      "綁進每筆紀錄的加密：紀錄被挪到別的串流、別的位置或別的輪次，就解不開了。",
      "各レコードの暗号化に結び付ける。別のストリーム、位置、世代に移されたレコードは復号できない。",
    ],
    src: `${P}/record.rs:188-195`,
  },
  "record.seq": {
    kind: "value",
    known: STORED,
    says: [
      "The record's position. Readers want exactly the next one, so a record dropped, replayed or reordered shows.",
      "记录的位置。读者只要恰好下一条，所以记录被丢、被重放或被调序都会露馅。",
      "紀錄的位置。讀者只要恰好下一筆，所以紀錄被丟、被重放或被調序都會露餡。",
      "レコードの位置。読み手はちょうど次のものしか受け取らないので、欠落も再送も順序の入れ替えも露見する。",
    ],
    src: `${P}/record.rs:127-128`,
  },
  "stream.epoch": {
    kind: "value",
    known: STORED,
    rel: ["stream.MK"],
    says: [
      "The content key epoch the stream key comes from (the e in MK_e).",
      "流钥出自内容密钥的哪一轮（MK_e 里的 e）。",
      "串流金鑰出自內容金鑰的哪一輪（MK_e 裡的 e）。",
      "ストリーム鍵の元になったコンテンツ鍵の世代（MK_e の e）。",
    ],
    src: `${P}/record.rs:188-195`,
  },
  "record.link": {
    kind: "value",
    known: STORED,
    says: [
      "Running hash over the stream. A signed head carries the latest one, so a record removed or replaced shows.",
      "整条流的累积哈希。签名流头带着最新的那个，所以记录被删或被换都会露馅。",
      "整條串流的累積雜湊。簽章串流頭帶著最新的那個，所以紀錄被刪或被換都會露餡。",
      "ストリーム全体の累積ハッシュ。署名付きのヘッドが最新の値を持つので、レコードの削除や差し替えは露見する。",
    ],
    src: `${P}/record.rs:243-251`,
  },
  "record.link-prev": {
    kind: "value",
    known: STORED,
    rel: ["record.link"],
    says: [
      "The link up to the record before; each record also carries it inside its encrypted plaintext.",
      "到前一条记录为止的累积哈希；每条记录的加密明文里也带着它。",
      "到前一筆紀錄為止的累積雜湊；每筆紀錄的加密明文裡也帶著它。",
      "1 つ前のレコードまでの累積ハッシュ。各レコードは暗号化された平文の中にもこれを持つ。",
    ],
    src: `${P}/record.rs:16-18`,
  },
  "record.record": {
    kind: "value",
    known: STORED,
    says: [
      "The record as stored: its seq, epoch, nonce and ciphertext.",
      "存着的那条记录：序号、轮次、nonce 和密文。",
      "存著的那筆紀錄：序號、輪次、nonce 和密文。",
      "保存されている形のレコード：連番、世代、nonce、暗号文。",
    ],
    src: `${P}/record.rs:197-205`,
  },
  "stream.buckets": {
    kind: "label",
    known: ALL,
    says: [
      "The sizes in bytes every record and page is padded up to before encryption, so its stored size says little.",
      "每条记录和每页在加密前补齐到的字节数，所以存下来的大小说明不了多少。",
      "每筆紀錄和每頁在加密前補齊到的位元組數，所以存下來的大小說明不了多少。",
      "暗号化の前に各レコードとページを埋めて揃えるバイト数。保存されたサイズからはほとんど何もわからない。",
    ],
    src: `${P}/record.rs:95-97`,
  },
  "stream.head": {
    kind: "value",
    known: STORED,
    says: [
      "The signed state of a stream. Readers release only records a head covers; a head past what arrived means records are held back.",
      "流的签名状态。读者只放出流头覆盖到的记录；流头比收到的多，就说明有记录被扣着。",
      "串流的簽章狀態。讀者只放出串流頭涵蓋到的紀錄；串流頭比收到的多，就說明有紀錄被扣著。",
      "ストリームの署名付きの状態。読み手はヘッドが含むレコードしか出さない。届いた分より先のヘッドは、レコードが差し止められている印。",
    ],
    src: `${P}/record.rs:19-27`,
  },
  "host.key": {
    kind: "secret",
    known: OTHER,
    says: [
      "The private identity key of the device that runs AI. A removed device may still hold an old stream key, but not this.",
      "可运行 AI 的设备的身份私钥。被移除的设备也许还留着旧的流钥，但没有这个。",
      "可執行 AI 的裝置的身分私鑰。被移除的裝置也許還留著舊的串流金鑰，但沒有這個。",
      "AI を実行できるデバイスの ID の秘密鍵。削除されたデバイスが古いストリーム鍵を持っていても、これは持っていない。",
    ],
    src: `${P}/record.rs:408-411`,
  },
  "head.next-seq": {
    kind: "value",
    known: STORED,
    says: [
      "How many records the stream holds: the seq the next record will get.",
      "流里有多少条记录：也就是下一条会拿到的序号。",
      "串流裡有多少筆紀錄：也就是下一筆會拿到的序號。",
      "ストリームにあるレコードの数。次のレコードが受け取る連番でもある。",
    ],
    src: `${P}/record.rs:381-388`,
  },
  "head.time": {
    kind: "value",
    known: STORED,
    says: [
      "When it was signed; the newest head's time is shown as “as of”.",
      "签名的时间；最新流头的时间显示为“截至”。",
      "簽章的時間；最新串流頭的時間顯示為「截至」。",
      "署名した時刻。最新のヘッドの時刻が「時点」として表示される。",
    ],
    src: `${P}/record.rs:19-27`,
  },
  "page.page": {
    kind: "value",
    known: STORED,
    says: [
      "A snapshot page of older history, sealed under the stream key and padded like a record.",
      "旧历史的一个快照页，用流钥封装，像记录一样补齐大小。",
      "舊歷史的一個快照頁，用串流金鑰封裝，像紀錄一樣補齊大小。",
      "古い履歴のスナップショットページ。ストリーム鍵で封印し、レコードと同じようにサイズを揃える。",
    ],
    src: `${P}/stream.rs:244-273`,
  },
  "page.aad": {
    kind: "value",
    known: STORED,
    says: ["Binds the page to its stream and epoch.", "把页绑在它的流和轮次上。", "把頁綁在它的串流和輪次上。", "ページをそのストリームと世代に結び付ける。"],
    src: `${P}/stream.rs:172-178`,
  },
  "page.name": {
    kind: "value",
    known: STORED,
    says: [
      "The page's name: the SHA-256 of its stored bytes. Change one byte and the name no longer matches.",
      "页的名字：它存储字节的 SHA-256。改一个字节，名字就对不上了。",
      "頁的名字：它儲存位元組的 SHA-256。改一個位元組，名字就對不上了。",
      "ページの名前：保存されたバイト列の SHA-256。1 バイト変えれば名前と一致しなくなる。",
    ],
    src: `${P}/stream.rs:161-170`,
  },
  "stream.manifest": {
    kind: "value",
    known: STORED,
    says: [
      "The signed list of a stream's pages, over a signed head. Readers start from it and check each page's name against it.",
      "一个流的页的签名列表，签在一个签名流头之上。读者从它开始，逐页核对名字。",
      "一個串流的頁的簽章列表，簽在一個簽章串流頭之上。讀者從它開始，逐頁核對名字。",
      "署名付きヘッドの上に署名した、ストリームのページの一覧。読み手はここから始め、各ページの名前を照合する。",
    ],
    src: `${P}/stream.rs:282-348`,
  },
  "manifest.count": {
    kind: "value",
    known: STORED,
    says: ["How many page names follow (at most 1024).", "后面跟着几个页名（最多 1024 个）。", "後面跟著幾個頁名（最多 1024 個）。", "後に続くページ名の数（最大 1024）。"],
    src: `${P}/stream.rs:93-95`,
  },
  "manifest.pages": {
    kind: "value",
    known: STORED,
    rel: ["page.name"],
    says: ["The pages' names, in order.", "各页的名字，按顺序。", "各頁的名字，按順序。", "ページの名前を順に並べたもの。"],
    src: `${P}/stream.rs:296-304`,
  },

  // ---- 6.2–6.5 identity renewal (not live yet)
  "rot.proof": {
    kind: "value",
    known: STORED,
    says: [
      "What both identity keys sign. Each verifier rebuilds it with the old keys from its own chain.",
      "新旧两把身份密钥都要签的内容。每个验证者都用自己链上的旧钥把它重建出来。",
      "新舊兩把身分金鑰都要簽的內容。每個驗證者都用自己鏈上的舊金鑰把它重建出來。",
      "2 つの ID 鍵の両方が署名する内容。検証者はそれぞれ自分のチェーンの古い鍵からこれを組み立て直す。",
    ],
    src: `${P}/chain.rs:345-374`,
  },
  "rot.staged-seq": {
    kind: "value",
    known: STORED,
    says: ["The chain position the proof was made against.", "这份证明是对着链上哪个位置做的。", "這份證明是對著鏈上哪個位置做的。", "この証明がチェーンのどの位置に対して作られたか。"],
    src: `${P}/chain.rs:306-309`,
  },
  "rot.staged-head": {
    kind: "value",
    known: STORED,
    rel: ["chain.hash"],
    says: [
      "The hash of that entry; verifiers check it against their own chain at that position.",
      "那条条目的哈希；验证者拿自己链上同一位置来核对。",
      "那筆項目的雜湊；驗證者拿自己鏈上同一位置來核對。",
      "そのエントリのハッシュ。検証者は自分のチェーンの同じ位置と照合する。",
    ],
    src: `${P}/chain.rs:306-309`,
  },
  "rot.old-sign": {
    kind: "public",
    known: ALL,
    says: [
      "The current Ed25519 public key of the device that runs AI, as on the chain.",
      "可运行 AI 的设备现在的 Ed25519 公钥，和链上一致。",
      "可執行 AI 的裝置現在的 Ed25519 公鑰，和鏈上一致。",
      "AI を実行できるデバイスの現在の Ed25519 公開鍵（チェーンのとおり）。",
    ],
    src: `${P}/chain.rs:319-335`,
  },
  "rot.old-dh": {
    kind: "public",
    known: STORED,
    says: ["Its current X25519 public key, as on the chain.", "它现在的 X25519 公钥，和链上一致。", "它現在的 X25519 公鑰，和鏈上一致。", "現在の X25519 公開鍵（チェーンのとおり）。"],
    src: `${P}/chain.rs:319-335`,
  },
  "rot.new-sign": {
    kind: "public",
    known: ALL,
    says: [
      "The new Ed25519 public key. No device on the chain may hold it, or ever have held it.",
      "新的 Ed25519 公钥。链上任何设备现在和过去都不能持有它。",
      "新的 Ed25519 公鑰。鏈上任何裝置現在和過去都不能持有它。",
      "新しい Ed25519 公開鍵。チェーン上のどのデバイスも、今も過去も持っていてはならない。",
    ],
    src: `${P}/chain.rs:76-87`,
  },
  "rot.new-dh": {
    kind: "public",
    known: STORED,
    says: [
      "The new X25519 public key: later keys are sealed to it, and channels use it.",
      "新的 X25519 公钥：之后的钥封装给它，通道也用它。",
      "新的 X25519 公鑰：之後的金鑰封裝給它，通道也用它。",
      "新しい X25519 公開鍵。以後の鍵はこれに封印され、チャネルもこれを使う。",
    ],
    src: `${P}/chain.rs:299-317`,
  },
  "rot.nonce": {
    kind: "value",
    known: STORED,
    says: [
      "The full device's random nonce for this request, so an old proof cannot be replayed.",
      "完整设备为这次请求给的随机 nonce，所以旧的证明没法重放。",
      "完整裝置為這次請求給的隨機 nonce，所以舊的證明沒辦法重放。",
      "フルデバイスがこの要求のために出す乱数の nonce。古い証明は再利用できない。",
    ],
    src: `${P}/chain.rs:310-311`,
  },
  "rot.assurance": {
    kind: "value",
    known: STORED,
    says: ["0: automatic. 1: confirmed on the device with its code.", "0：自动。1：用代码在那台设备上确认过。", "0：自動。1：用代碼在那台裝置上確認過。", "0：自動。1：そのデバイスでコードを使って確認済み。"],
    src: `${P}/chain.rs:263-291`,
  },
  "rot.old-sig": {
    kind: "value",
    known: STORED,
    says: [
      "The old key's signature over the proof: whoever holds the current identity agrees.",
      "旧钥对证明的签名：握着现有身份的一方同意。",
      "舊金鑰對證明的簽章：握著現有身分的一方同意。",
      "古い鍵による証明への署名：今の ID を持つ側が同意している。",
    ],
    src: `${P}/chain.rs:337-342`,
  },
  "rot.new-sig": {
    kind: "value",
    known: STORED,
    says: [
      "The new key's signature over the same proof: whoever holds the new key agrees too.",
      "新钥对同一份证明的签名：握着新钥的一方也同意。",
      "新金鑰對同一份證明的簽章：握著新金鑰的一方也同意。",
      "新しい鍵による同じ証明への署名：新しい鍵を持つ側も同意している。",
    ],
    src: `${P}/chain.rs:337-342`,
  },
  "rot.old-key": {
    kind: "secret",
    known: OTHER,
    says: [
      "The current Ed25519 private key of the device that runs AI, from its identity seed.",
      "可运行 AI 的设备现在的 Ed25519 私钥，出自它的身份种子。",
      "可執行 AI 的裝置現在的 Ed25519 私鑰，出自它的身分種子。",
      "AI を実行できるデバイスの現在の Ed25519 秘密鍵。ID のシードから作る。",
    ],
    src: `${P}/chain.rs:337-342`,
  },
  "rot.new-key": {
    kind: "secret",
    known: OTHER,
    says: [
      "The new Ed25519 private key, from a new seed the device that runs AI keeps beside the old one until the entry is on its own chain.",
      "新的 Ed25519 私钥，出自一颗新种子；可运行 AI 的设备把它和旧种子并排放着，直到条目上了自己的链。",
      "新的 Ed25519 私鑰，出自一顆新種子；可執行 AI 的裝置把它和舊種子並排放著，直到項目上了自己的鏈。",
      "新しい Ed25519 秘密鍵。新しいシードから作り、AI を実行できるデバイスはエントリが自分のチェーンに載るまで古いシードと並べて保管する。",
    ],
    src: "docs/host-identity-rotation.md:170-174",
  },
  "chain.RotateHostKey": {
    kind: "value",
    known: STORED,
    says: [
      "Chain entry kind 20: renews the identity key and keeps the id. It carries the new keys, the position, the nonce, the assurance and both signatures.",
      "第 20 种链条目：更换身份密钥、保留 id。里面有新钥、位置、nonce、保障级别和两个签名。",
      "第 20 種鏈項目：更換身分金鑰、保留 id。裡面有新金鑰、位置、nonce、保障等級和兩個簽章。",
      "種類 20 のチェーンのエントリ：id を保ったまま ID 鍵を更新する。新しい鍵、位置、nonce、保証レベル、2 つの署名を持つ。",
    ],
    src: `${P}/chain.rs:1061-1071`,
  },
  "chain.KeyEpochs": {
    kind: "value",
    known: STORED,
    says: [
      "Kind 10: commits key epochs. Here it must come straight after, with the next content key of that device.",
      "第 10 种：提交钥匙轮次。这里它必须紧跟在后面，提交那台设备的下一把内容密钥。",
      "第 10 種：提交金鑰輪次。這裡它必須緊跟在後面，提交那台裝置的下一把內容金鑰。",
      "種類 10：鍵の世代をコミットする。ここではすぐ後に続き、そのデバイスの次のコンテンツ鍵をコミットしなければならない。",
    ],
    src: `${P}/chain.rs:76-81`,
  },
  "rot.next": {
    kind: "value",
    known: STORED,
    rel: ["key.commit"],
    says: [
      "The next content key epoch's commitment; that key is sealed only to the new X25519 key.",
      "下一轮内容密钥的承诺；那把钥只封装给新的 X25519 钥。",
      "下一輪內容金鑰的承諾；那把金鑰只封裝給新的 X25519 金鑰。",
      "次の世代のコンテンツ鍵のコミットメント。その鍵は新しい X25519 鍵にだけ封印される。",
    ],
    src: `${P}/chain.rs:470-488`,
  },
  "rot.mac": {
    kind: "value",
    known: ENDS,
    says: [
      "Proves the device that runs AI holds the new X25519 private key, which a signature cannot show.",
      "证明可运行 AI 的设备握有新的 X25519 私钥——这一点签名证明不了。",
      "證明可執行 AI 的裝置握有新的 X25519 私鑰——這一點簽章證明不了。",
      "AI を実行できるデバイスが新しい X25519 秘密鍵を持つことの証明。これは署名では示せない。",
    ],
    src: `${P}/chain.rs:376-405`,
  },
  "rot.new-dh-priv": {
    kind: "secret",
    known: OTHER,
    rel: ["rot.new-dh"],
    says: [
      "The new X25519 private key. Your device computes the same secret from its one-off private key and the new public key.",
      "新的 X25519 私钥。你的设备用自己的临时私钥配新公钥，算出同一份秘密。",
      "新的 X25519 私鑰。你的裝置用自己的臨時私鑰配新公鑰，算出同一份秘密。",
      "新しい X25519 秘密鍵。あなたのデバイスは自分の使い捨ての秘密鍵と新しい公開鍵から同じ秘密を計算する。",
    ],
    src: `${P}/chain.rs:389-405`,
  },
  "rot.eph": {
    kind: "public",
    known: ENDS,
    says: [
      "Your full device's one-off X25519 public key, sent with the request inside the end-to-end channel.",
      "你这台完整设备的一次性 X25519 公钥，随请求在端到端通道里发出。",
      "你這台完整裝置的一次性 X25519 公鑰，隨請求在端對端通道裡送出。",
      "あなたのフルデバイスの使い捨ての X25519 公開鍵。要求と一緒にエンドツーエンドのチャネルで送る。",
    ],
    src: `${P}/chain.rs:396-405`,
  },
  "rot.ack": {
    kind: "value",
    known: ["other", "hub"],
    says: [
      "At each sign-in with the old key during the window, the new key confirms the staged renewal. A sign-in without it voids the staging.",
      "窗口内每次用旧钥登录，都要由新钥确认这次暂存的更换。少了它，暂存就作废。",
      "窗口內每次用舊金鑰登入，都要由新金鑰確認這次暫存的更換。少了它，暫存就作廢。",
      "期間中に古い鍵でサインインするたびに、新しい鍵が保留中の更新を確認する。確認のないサインインがあれば保留は無効になる。",
    ],
    src: `${P}/chain.rs:420-442`,
  },
  "rot.login-nonce": {
    kind: "value",
    known: ["other", "hub"],
    says: [
      "The Hub's challenge this connection signed in with, so the confirmation is good for this connection only.",
      "这条连接登录时用的 Hub 挑战，所以这份确认只对这条连接有效。",
      "這條連線登入時用的 Hub 挑戰，所以這份確認只對這條連線有效。",
      "この接続がサインインに使った Hub のチャレンジ。確認はこの接続にしか通用しない。",
    ],
    src: `${P}/chain.rs:420-425`,
  },
  "rot.code": {
    kind: "value",
    known: STORED,
    says: [
      "The code shown on the device that runs AI and typed on your full device, which computes it from the proof. Not secret: it proves which screen you read.",
      "可运行 AI 的设备上显示、你在完整设备上输入的代码；完整设备从证明算出它。它不是秘密：它证明你读的是哪块屏幕。",
      "可執行 AI 的裝置上顯示、你在完整裝置上輸入的代碼；完整裝置從證明算出它。它不是祕密：它證明你讀的是哪塊螢幕。",
      "AI を実行できるデバイスに表示され、あなたがフルデバイスに入力するコード。フルデバイスは証明から計算する。秘密ではなく、どの画面を読んだかの証明になる。",
    ],
    src: `${P}/chain.rs:407-418`,
  },
  "rot.dispute": {
    kind: "value",
    known: STORED,
    says: [
      "Signed with the retired key: “this renewal was not mine.” Devices that verify it isolate the device that runs AI.",
      "用退役钥签的一句话：“这次更换不是我。”验证了它的设备会隔离那台可运行 AI 的设备。",
      "用退役金鑰簽的一句話：「這次更換不是我。」驗證了它的裝置會隔離那台可執行 AI 的裝置。",
      "退役した鍵で署名する一言：「この更新は私ではない」。これを検証したデバイスは、その AI を実行できるデバイスを隔離する。",
    ],
    src: `${P}/chain.rs:444-468`,
  },
  "rot.retired": {
    kind: "secret",
    known: OTHER,
    says: [
      "The private key the disputed renewal retired, still held by the real device.",
      "被这次有争议的更换退役的那把私钥，真设备还留着。",
      "被這次有爭議的更換退役的那把私鑰，真裝置還留著。",
      "争われている更新で退役させられた秘密鍵。本物のデバイスがまだ持っている。",
    ],
    src: `${P}/chain.rs:464-468`,
  },
  "rot.seq": {
    kind: "value",
    known: STORED,
    says: [
      "The sequence number of the disputed RotateHostKey entry.",
      "有争议的那条 RotateHostKey 条目的序号。",
      "有爭議的那筆 RotateHostKey 項目的序號。",
      "争われている RotateHostKey エントリの連番。",
    ],
    src: `${P}/chain.rs:444-454`,
  },
  "rot.entry-hash": {
    kind: "value",
    known: STORED,
    rel: ["chain.hash"],
    says: [
      "That entry's chain hash: a dispute can exist only after the entry, with no clocks compared.",
      "那条条目的链哈希：争议只能出现在条目之后，不用比较任何时钟。",
      "那筆項目的鏈雜湊：爭議只能出現在項目之後，不用比較任何時鐘。",
      "そのエントリのチェーン上のハッシュ。異議はエントリの後にしか作れず、時計を比べる必要がない。",
    ],
    src: `${P}/chain.rs:444-454`,
  },
};

/** Who "your device" and "the other device" are in a block. */
export interface Roles {
  me: Words;
  other: Words;
}

export interface FormulaLine {
  text: string;
  /** This line's own meanings: a symbol, or one meaning per occurrence in the line. */
  terms?: Readonly<Record<string, string | readonly string[]>>;
}

export interface FormulaBlock {
  /** The parties' roles; a block without them (labels only) has no "view as". */
  roles?: Roles;
  /** The block's meanings; a number not named here takes `#` if given. */
  terms?: Readonly<Record<string, string>>;
  lines: readonly (string | FormulaLine)[];
}

/** Meanings that hold on every line unless a block or a line says otherwise. */
export const COMMON: Readonly<Record<string, string>> = {
  Ed25519: "fn.ed25519",
  "SHA-256": "fn.sha256",
  "HKDF-SHA256": "fn.hkdf",
  X25519: "fn.x25519",
  "AES-256-GCM": "fn.aesgcm",
  "HMAC-SHA256": "fn.hmac",
  base32: "fn.base32",
  "‖": "op.concat",
  "|": "op.field",
  "←": "op.signed",
  "→": "op.as",
  "[0..5]": "op.slice",
  account: "id.account",
  device: "id.device",
  host: "id.host",
  salt: "hkdf.salt",
  info: "hkdf.info",
  stream: "stream.id",
  stream_id: "stream.id",
  admin: "admin.key",
  proof: "rot.proof",
  new_sign: "rot.new-sign",
  new_dh: "rot.new-dh",
  old: "rot.old-key",
  new: "rot.new-key",
  RotateHostKey: "chain.RotateHostKey",
  KeyEpochs: "chain.KeyEpochs",
  // The Noise suite's name, split at its underscores.
  Noise: "noise.Noise",
  IK: "noise.IK",
  "25519": "fn.x25519",
  AESGCM: "fn.aesgcm",
  SHA256: "fn.sha256",
};

/** Every label, quoted in a formula (without its `\0`) or listed bare. */
export const LABELS: Readonly<Record<string, string>> = {
  "rh/v1": "label.rh-v1",
  "rh/v1 chain-entry": "label.chain-entry",
  "rh/v1 chain-hash": "label.chain-hash",
  "rh/v1 chain-attest": "label.chain-attest",
  "rh/v1 seal-auth": "label.seal-auth",
  "rh/v1 seal": "label.seal",
  "rh/v1 key-commit": "label.key-commit",
  "rh/v1 direct": "label.direct",
  "rh/v1 stream-key": "label.stream-key",
  "rh/v1 record": "label.record",
  "rh/v1 record-link": "label.record-link",
  "rh/v1 stream-head": "label.stream-head",
  "rh/v1 page": "label.page",
  "rh/v1 stream-manifest": "label.stream-manifest",
  "rh/v1 stream-draft": "label.stream-draft",
  "rh/v1 hub-login": "label.hub-login",
  "rh/v1 push": "label.push",
  "rh/v1 recovery-proof": "label.recovery-proof",
  "rh/v1 passkey-proof": "label.passkey-proof",
  "rh/v1 relay-ticket": "label.relay-ticket",
  "rh/v1 relay-origin": "label.relay-origin",
  "rh/v1 host-rotate": "label.host-rotate",
  "rh/v1 rotate-confirm": "label.rotate-confirm",
  "rh/v1 rotate-ack": "label.rotate-ack",
  "rh/v1 rotate-sas": "label.rotate-sas",
  "rh/v1 rotate-dispute": "label.rotate-dispute",
};

const devices: Roles = {
  me: ["starts the channel", "发起通道的一方", "發起通道的一方", "チャネルを開く側"],
  other: ["the device that runs AI", "可运行 AI 的设备", "可執行 AI 的裝置", "AI を実行できるデバイス"],
};
const streams: Roles = {
  me: ["reads the copy", "读副本的一方", "讀副本的一方", "コピーを読む側"],
  other: ["the device that runs AI, which writes it", "写副本的可运行 AI 的设备", "寫副本的可執行 AI 的裝置", "書き込む側の AI を実行できるデバイス"],
};
const renewal: Roles = {
  me: ["a full device", "一台完整设备", "一台完整裝置", "フルデバイス"],
  other: ["the device that runs AI being renewed", "正在更换身份的可运行 AI 的设备", "正在更換身分的可執行 AI 的裝置", "ID を更新される AI を実行できるデバイス"],
};

/** The page's formula blocks, by the place they stand. */
export const BLOCKS = {
  chain: {
    roles: {
      me: ["signs the entry", "签这条条目的一方", "簽這筆項目的一方", "エントリに署名する側"],
      other: ["checks it", "验证它的一方", "驗證它的一方", "それを検証する側"],
    },
    terms: {
      entry: "chain.entry",
      version: "chain.version",
      seq: "chain.seq",
      prev: "chain.prev",
      time: "chain.time",
      signer: "chain.signer",
      kind: "chain.kind",
      action: "chain.action",
      signature: "chain.signature",
      "next.prev": "chain.hash",
    },
    lines: [
      "entry = version | account | seq | prev | time | signer | kind | action | signature",
      {
        text: 'signature = Ed25519(signer, "rh/v1 chain-entry\\0" ‖ version | account | seq | prev | time | signer | kind | action)',
        terms: { signer: ["chain.signer-key", "chain.signer"] },
      },
      'next.prev = SHA-256("rh/v1 chain-hash\\0" ‖ entry)',
    ],
  },
  attest: {
    roles: {
      me: ["a full device that attests", "作证的完整设备", "作證的完整裝置", "証明するフルデバイス"],
      other: ["a device that runs AI, checking freshness", "核对新鲜度的可运行 AI 的设备", "核對新鮮度的可執行 AI 的裝置", "鮮度を確かめる AI を実行できるデバイス"],
    },
    terms: {
      attest: "attest.sig",
      seq: "attest.seq",
      head: "attest.head",
      time: "attest.time",
      CONTROL_FRESHNESS_MS: "const.control",
      "7 d": "const.control",
      MANAGE_FRESHNESS_MS: "const.manage",
      "12 h": "const.manage",
      ATTEST_EVERY_MS: "const.attest-every",
      "3 h": "const.attest-every",
    },
    lines: [
      'attest = Ed25519(admin, "rh/v1 chain-attest\\0" ‖ account | device | seq | head | time)',
      "CONTROL_FRESHNESS_MS = 7 d · MANAGE_FRESHNESS_MS = 12 h · ATTEST_EVERY_MS = 3 h",
    ],
  },
  seal: {
    roles: {
      me: ["the sender", "发送方", "傳送方", "送信者"],
      other: ["the recipient", "接收方", "接收方", "受信者"],
    },
    terms: {
      kek: "seal.kek",
      R: "seal.R",
      ctx: "seal.ctx",
      purpose: "seal.purpose",
      sender: "seal.sender",
      recipient: "seal.recipient",
      epoch: "seal.epoch",
      AAD: "seal.aad",
      commit: "key.commit",
      kind: "key.kind",
      id: "key.id",
      key: "key.key",
    },
    lines: [
      {
        text: 'kek = HKDF-SHA256(salt = "rh/v1 seal-auth", X25519(e, R) ‖ X25519(S, R), info = e ‖ S ‖ R ‖ ctx)',
        terms: { e: ["seal.e-priv", "seal.e-pub"], S: ["seal.S-priv", "seal.S-pub"] },
      },
      'ctx = "rh/v1 seal\\0" ‖ purpose | account | sender | recipient | epoch [| host];  AES-256-GCM(kek, AAD = ctx)',
      'commit = SHA-256("rh/v1 key-commit\\0" ‖ kind | account | id | epoch | key)',
    ],
  },
  noise: {
    roles: devices,
    terms: { prologue: "noise.prologue" },
    lines: ["Noise_IK_25519_AESGCM_SHA256", 'prologue = "rh/v1\\0" ‖ account | device | host'],
  },
  direct: {
    roles: devices,
    terms: {
      prologue: "direct.prologue",
      channel_hash: "direct.channel-hash",
      nonce: "direct.nonce",
      device_fp: "direct.device-fp",
      host_fp: "direct.host-fp",
    },
    lines: ['prologue = "rh/v1 direct\\0" ‖ account | device | host | channel_hash | nonce | device_fp | host_fp'],
  },
  stream: {
    roles: streams,
    terms: {
      SK: "stream.SK",
      MK_e: "stream.MK",
      AAD: "record.aad",
      seq: "record.seq",
      epoch: "stream.epoch",
      link_n: "record.link",
      "link_n−1": "record.link-prev",
      record_n: "record.record",
      BUCKETS: "stream.buckets",
      "#": "stream.buckets",
    },
    lines: [
      'SK = HKDF-SHA256(salt = "rh/v1 stream-key", MK_e, info = stream_id)',
      'AAD = "rh/v1 record\\0" ‖ stream ‖ seq ‖ epoch',
      'link_n = SHA-256("rh/v1 record-link\\0" ‖ stream ‖ link_n−1 ‖ record_n)',
      "BUCKETS = [256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 61440]",
    ],
  },
  head: {
    roles: streams,
    terms: { head: "stream.head", host: "host.key", next_seq: "head.next-seq", epoch: "stream.epoch", link: "record.link", time: "head.time" },
    lines: ['head = Ed25519(host, "rh/v1 stream-head\\0" ‖ stream | next_seq | epoch | link | time)'],
  },
  pages: {
    roles: streams,
    terms: {
      page: "page.page",
      SK: "stream.SK",
      AAD: "page.aad",
      epoch: "stream.epoch",
      name: "page.name",
      manifest: "stream.manifest",
      host: "host.key",
      head: "stream.head",
      count: "manifest.count",
      "pages[]": "manifest.pages",
    },
    lines: [
      'page = AES-256-GCM(SK, AAD = "rh/v1 page\\0" ‖ stream ‖ epoch);  name = SHA-256(page)',
      'manifest = Ed25519(host, "rh/v1 stream-manifest\\0" ‖ head | count | pages[])',
    ],
  },
  rotate: {
    roles: renewal,
    terms: {
      staged_seq: "rot.staged-seq",
      staged_head: "rot.staged-head",
      old_sign: "rot.old-sign",
      old_dh: "rot.old-dh",
      nonce: "rot.nonce",
      assurance: "rot.assurance",
      old_sig: "rot.old-sig",
      new_sig: "rot.new-sig",
      next: "rot.next",
    },
    lines: [
      "proof = account | host | staged_seq | staged_head | old_sign | old_dh | new_sign | new_dh | nonce | assurance",
      'old_sig = Ed25519(old, "rh/v1 host-rotate\\0" ‖ proof)',
      'new_sig = Ed25519(new, "rh/v1 host-rotate\\0" ‖ proof)',
      '[RotateHostKey{host, new_sign, new_dh, …, old_sig, new_sig}, KeyEpochs{host: next}] ← admin, "rh/v1 chain-entry\\0"',
    ],
  },
  confirm: {
    roles: renewal,
    terms: { mac: "rot.mac", new_dh: "rot.new-dh-priv", eph: "rot.eph", ack: "rot.ack", login_nonce: "rot.login-nonce", code: "rot.code", "XXXX-XXXX": "rot.code" },
    lines: [
      'mac = HMAC-SHA256(X25519(new_dh, eph), "rh/v1 rotate-confirm\\0" ‖ proof)',
      'ack = Ed25519(new, "rh/v1 rotate-ack\\0" ‖ account | host | SHA-256(proof) | login_nonce)',
      'code = base32(SHA-256("rh/v1 rotate-sas\\0" ‖ proof)[0..5])  →  XXXX-XXXX',
    ],
  },
  dispute: {
    roles: renewal,
    terms: { dispute: "rot.dispute", retired: "rot.retired", seq: "rot.seq", entry_hash: "rot.entry-hash" },
    lines: ['dispute = Ed25519(retired, "rh/v1 rotate-dispute\\0" ‖ account | host | seq | entry_hash | new_sign)'],
  },
  labels: {
    lines: [
      "rh/v1 chain-entry · rh/v1 chain-attest · rh/v1 stream-head · rh/v1 stream-manifest · rh/v1 stream-draft",
      "rh/v1 hub-login · rh/v1 push · rh/v1 recovery-proof · rh/v1 passkey-proof",
      "rh/v1 relay-ticket · rh/v1 relay-origin · rh/v1 host-rotate · rh/v1 rotate-ack",
    ],
  },
} as const satisfies Record<string, FormulaBlock>;

export type BlockId = keyof typeof BLOCKS;
