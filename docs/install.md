# Installing Droppen

Install Droppen on the computer where Claude Code or Codex runs. Your phone and
other devices need nothing installed: they open
[my.droppen.app](https://my.droppen.app) in a browser.
[droppen.app/download](https://www.droppen.app/download) walks through the
same steps.

## Releases

Releases are published as this repository's
[releases](https://github.com/getdroppen/droppen/releases): the desktop app,
the server for macOS and Linux, the installer, `SHA256SUMS`, `LICENSE` and
`THIRD_PARTY_NOTICES`. They are built here by GitHub Actions, and each file
has a build provenance attestation ([Verify a download](#verify-a-download)).
Releases up to v0.2.126 are at
[getdroppen/releases](https://github.com/getdroppen/releases).

## Desktop app (macOS 13 or later)

Download `Droppen-<version>.dmg` from the
[latest release](https://github.com/getdroppen/droppen/releases/latest) and
install it. The app starts its own copy of the local program, logs in to it for
you and keeps itself up to date.

## Server (macOS and Linux)

For a computer you use in the background, or one with no screen:

```sh
curl -fsSL https://droppen.app/install.sh | sh
```

It downloads the latest release, checks it against the release's `SHA256SUMS`,
creates the first local account, installs the program to
`~/.local/share/remote-hand/bin/` (the command is `droppen`) and runs it on
`127.0.0.1:8700` (or the next free port) as a launchd user agent
(`app.droppen.server`) on macOS, or a systemd user service (`droppen.service`)
on Linux (x86_64 and aarch64, glibc 2.28 or later). Run it again to upgrade;
`curl -fsSL https://droppen.app/install.sh | sh -s -- --uninstall` removes it
and keeps your data. `droppen.app/install.sh` serves the `install.sh` of the
release Droppen Hub was built from, byte for byte that release's file
(`https://github.com/getdroppen/droppen/releases/download/<tag>/install.sh`;
its sha256 is `install_sh` in `https://droppen.app/.well-known/droppen-build`),
never a redirect; the source is [`scripts/install.sh`](../scripts/install.sh),
which lists every option in its header.

To join the computer to your Droppen account in the same step, pass the
one-time code droppen.app shows under
*my devices → add a device that runs AI → show a one-time code*:

```sh
curl -fsSL https://droppen.app/install.sh | DROPPEN_CODE=<code> sh
```

The local program listens on loopback only. Reach it from other devices
through Droppen, or through an SSH tunnel or Tailscale. Do not expose its port
to the Internet: whoever reaches it can run commands as you. It answers to IP
addresses, `localhost` and local-network names (`mymac`, `*.local`, `*.lan`,
`*.home.arpa`, `*.internal`, a tailnet's `*.ts.net`); a reverse proxy or
tunnel that passes another name on (Caddy, a Cloudflare Tunnel) needs that
name in `DROPPEN_ALLOWED_HOSTS` (comma-separated). Local-network names are
whatever the network's own name resolution says, so a device on the same
network that controls it can still reach the routes that answer without a
session (sign-in, first-run setup, status); everything else needs a session.

## Windows

Droppen does not run on Windows yet. A browser on Windows can already use
[my.droppen.app](https://my.droppen.app) to control your other computers, and
the server runs in WSL with systemd turned on.

## Optional: VS Code in the web app

The rail's `+` → `vscode` opens VS Code in the browser through
[code-server](https://github.com/coder/code-server), which Droppen does not
install. To use it, install code-server on the computer the server runs on
(see [code-server's install guide](https://coder.com/docs/code-server/install)),
somewhere on the server's `PATH`. The service the installer sets up (the
launchd agent or the systemd user service) and the desktop app's server have
`~/.local/bin` on their `PATH`, which is where code-server's standalone install
(`--method=standalone`) puts it; `DROPPEN_SERVICE_PATH` sets another `PATH`
when you run the installer. The server looks for `code-server` each time an
editor is opened, so it needs no restart. Without it, opening VS Code says that
code-server is missing and links to the guide.

## Verify a download

Every file of a release from v0.2.127 on has a build provenance attestation.
With the [GitHub CLI](https://cli.github.com), check that this repository's
release workflow built a file for that very tag:

```sh
gh attestation verify <file> --repo getdroppen/droppen \
  --signer-workflow getdroppen/droppen/.github/workflows/release.yml \
  --source-ref refs/tags/<version>
```

Without the GitHub CLI, compare a file with its line in the release's
`SHA256SUMS` (`shasum -a 256 <file>`); the install script runs the same check
before it installs anything. [Releasing Droppen](releasing.md#verifying-a-download)
covers the rest: the Sigstore bundle, the desktop app's Developer ID signature
and notarisation, and the signed updates. The web app at droppen.app is checked
against the source with `node scripts/verify-web.mjs`
([web-integrity.md](web-integrity.md), in Chinese).
