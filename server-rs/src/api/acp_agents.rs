//! The ACP agents' routes (`droppen_provider_acp::agents`) through the router
//! the server builds, behind its login guard and the bridge: who reads a last
//! check's error.

use droppen_provider_acp::agents::{AcpAgentRegistry, AcpAgentUpdate, AcpCheck, AcpCheckState};

fn patch(command: Option<&str>, args: Option<&[&str]>, env: Option<&str>) -> AcpAgentUpdate {
    AcpAgentUpdate {
        name: None.into(),
        command: command.map(str::to_owned).into(),
        args: args.map(|args| args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>()).into(),
        env: env.map(str::to_owned).into(),
    }
}

/// A last check's error is the agent's own words. The list, which every
/// device may read, never carries it, whoever asks; the check's own route
/// does, and a device reaches that only when it manages this computer.
#[tokio::test]
async fn a_check_error_is_only_on_the_checks_own_route() {
    use droppen_agent::bridge::{refusal, Standing, View};
    use tower_service::Service;

    async fn get(router: &mut gotcha::axum::Router, path: &str, headers: &[(&str, &str)]) -> (u16, serde_json::Value) {
        let mut request = gotcha::axum::http::Request::get(path).header("host", "127.0.0.1:8700");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let mut request = request.body(gotcha::axum::body::Body::empty()).expect("request");
        let peer: std::net::SocketAddr = "127.0.0.1:50000".parse().expect("peer");
        request.extensions_mut().insert(gotcha::axum::extract::ConnectInfo(peer));
        let response = router.call(request).await.expect("infallible");
        let status = response.status().as_u16();
        let body = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("body");
        (status, serde_json::from_slice(&body).expect("json"))
    }

    let home = tempfile::tempdir().expect("tempdir");
    let (state, mut router) = crate::api::test_support::guarded_router(home.path(), Some(false)).await;
    let agents = AcpAgentRegistry::of(&state.paths);
    agents.update("x", &patch(Some("/x/agent"), None, None)).expect("saved");
    let failed = AcpCheck::ended(AcpCheckState::Failed, Some("the agent's own words".into()));
    assert!(agents.get("x").unwrap().record_check(&agents, &failed));
    let credential = state.drive.auth.internal_credential().to_string();
    let device = [
        (droppen_http::bridge::CREDENTIAL_HEADER, credential.as_str()),
        (droppen_http::bridge::DEVICE_HEADER, "d-1"),
    ];

    // This computer, and a device bridged in: how the check ended, not why.
    for headers in [&[][..], &device[..]] {
        let (status, list) = get(&mut router, "/api/acp/agents", headers).await;
        assert_eq!(status, 200, "{list:#}");
        let check = &list["agents"][0]["lastCheck"];
        assert_eq!(check["state"], "failed", "{list:#}");
        assert!(check["checkedAt"].is_string(), "{list:#}");
        assert!(check.get("error").is_none(), "{list:#}");
    }

    // The bridge lets every device list the agents, and only one that
    // manages this computer ask for the check in full.
    let plain = Standing {
        admin: false,
        view: View::Fresh,
        controls: true,
    };
    let admin = Standing { admin: true, ..plain };
    assert_eq!(refusal("GET", "/api/acp/agents", plain), None);
    assert_eq!(refusal("GET", "/api/acp/agents/x/check", plain).map(|refused| refused.answer().0), Some(403));
    assert_eq!(refusal("GET", "/api/acp/agents/x/check", admin), None);
    let (status, check) = get(&mut router, "/api/acp/agents/x/check", &device).await;
    assert_eq!((status, check["error"].as_str()), (200, Some("the agent's own words")), "{check:#}");

    agents.update("y", &patch(Some("/y/agent"), None, None)).expect("saved");
    assert_eq!(get(&mut router, "/api/acp/agents/y/check", &[]).await.0, 404, "never checked");
    assert_eq!(get(&mut router, "/api/acp/agents/z/check", &[]).await.0, 404, "no such agent");
}
