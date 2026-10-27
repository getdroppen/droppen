# Tour screenshots: the local app

The pictures of the local web app (the Mac app's window, and `droppen` on
Linux) that the homepage's product tour shows. They are taken of the real
server and the real web app built from this checkout, serving a made-up home
directory, in a throwaway pod.

## Regenerate

```sh
scripts/landing-shots/local/run.sh /tmp/droppen-tour/shots/local
```

It uses kubectl's current context and namespace (`KUBE_CONTEXT`,
`KUBE_NAMESPACE` override them; see the top of `run.sh` for the rest), takes
about five minutes in a fresh pod, and deletes the pod when it is done. Nothing
runs on the machine you start it from.

Out come 14 files, `<shot>-en-<theme>.png` with `theme` `light` | `dark`. The
homepage shows the English shots whatever the page's language. The scenario is
also written in Chinese: `shoot.sh lang zh-CN` in a pod kept with `KEEP_POD=1`
shoots that.

| shot | device | what it shows |
| ---- | ------ | ------------- |
| `app-session` | desktop | the session list in mixed states, and a Claude session's tool calls ending in a question waiting for an answer |
| `app-split` | desktop | a saved three-pane screen: a working Claude session, a Codex session, a terminal running a dev server |
| `app-terminal` | desktop | a terminal: `git log` and a test run |
| `app-new-session` | desktop | starting a session: the composer with a prompt, the directories below |
| `app-files` | desktop | the files view previewing a README |
| `phone-session` | phone | the question card and the composer on a phone |
| `phone-list` | phone | the session list on a phone |

Desktop is 1280×800 CSS px at 2×, about the width the homepage frames it at,
phone 390×844 at 3× (mobile emulation). No browser chrome: the homepage draws
the frames.

## What is real and what is scripted

Real: the server (`remote-hand-server serve`, authentication off), the web app
it embeds, its session list, transcripts, question card, terminals (a login
shell in a PTY), screens and files; `git`, and the test suite `npm test` runs
in the terminal.

Scripted: the agents. `cli/claude.mjs` and `cli/codex.mjs` are on the server's
`PATH` as `claude` and `codex`. The scripted `claude` speaks the CLI's
stream-json protocol and writes its transcript the way Claude Code does, but
plays turns from `scenario.mjs` instead of asking a model; one of them ends in
an `AskUserQuestion` the server holds open as a pending question, another in a
command that never returns, so that session stays working. The finished
sessions, Claude transcripts and Codex rollouts, are written by `seed.mjs`. The
dev server in the split screen is `vite.mjs`.

Everything in the scenario is invented: the user `dev` on host `macbook`, the
projects under `~/code`, every conversation (once in English, once in Chinese).

## Files

- `run.sh`: makes the pod, uploads the checkout, runs `shoot.sh`, copies the PNGs back.
- `shoot.sh`: inside the pod: build, then per language seed, serve, stage, capture.
  `shoot.sh lang zh-CN` in a pod kept with `KEEP_POD=1` re-shoots one language.
- `scenario.mjs`: the demo's content; `transcript.mjs`: it as the CLIs' files.
- `seed.mjs`, `stage.mjs`, `capture.mjs`: the home on disk, the live state through the API, the pictures.
