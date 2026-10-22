# Security policy

## Reporting a vulnerability

Report vulnerabilities privately, with GitHub's private vulnerability
reporting: open this repository's **Security** tab and choose **Report a
vulnerability**. Only you and the maintainers can see the report. If you
cannot use GitHub, e-mail **security@droppen.app**.

Please do not open a public issue, pull request or discussion about a
vulnerability, and do not disclose it elsewhere until a fix has been released.

A useful report says:

- what is affected: the component (local program, web app, desktop app, relay,
  installer, droppen.app) and its version (`droppen --version`, or
  *About Droppen* in the desktop app);
- how to reproduce it, ideally with a proof of concept;
- what an attacker gains, and from which position (another device on the
  account, the Hub, a relay, the network, a web page, another user on the same
  computer);
- a suggested fix, if you have one.

We will follow up where you reported it (the advisory, or by e-mail), keep you
informed there while we work on a fix, and are glad to credit you when it is
published.

## Scope

In scope:

- the code in this repository: the local program (including the agent and the
  local gateway), the web app (including its cloud mode), the desktop app, the
  relay, the protocol and client crates, and the plugin SDKs;
- the hosted service at droppen.app: the Hub, the web app it serves and the
  relays it lists;
- the installer (`https://droppen.app/install.sh`) and the release files at
  [getdroppen/droppen's releases](https://github.com/getdroppen/droppen/releases)
  (and, up to v0.2.127, [getdroppen/releases](https://github.com/getdroppen/releases)).

The design assumes a malicious Hub. A way for the Hub, a relay or someone on
the network to read or change what should be end-to-end encrypted, or for a
device that is not on your account to act on your computer, is a vulnerability
even though the Hub's own code is not in this repository.

Known limitations, already documented, are not vulnerabilities in themselves
(new ways to exploit them are welcome):

- the Hub can serve a modified web app to a targeted user; this is why the
  local web app and the desktop app exist
  ([docs/web-integrity.md](docs/web-integrity.md));
- the Hub sees metadata such as which device talks to which computer, and the
  sizes and timing of traffic
  ([docs/cross-machine-architecture.md](docs/cross-machine-architecture.md) §2
  and §7);
- the local program runs AI agents and shells as your user, so anyone who can
  reach its port, or run code as your user on that computer, can act as you.

When testing against droppen.app, use only accounts you own, do not access or
change anyone else's data, and do not degrade the service for others
(no load or denial-of-service testing).
