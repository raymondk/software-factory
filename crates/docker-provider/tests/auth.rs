//! The provider token gates starting and stopping workers; `/status` stays open. Needs no Docker.

use docker_provider::{api, config::Config, AppState};
use reqwest::StatusCode;
use serde_json::json;

async fn serve() -> String {
    let config = Config::parse(
        "[provider]\nlisten = \"127.0.0.1:0\"\nmax_workers = 1\ntoken = \"provider-secret\"\n\
         [agents.a]\nimage = \"img\"\nmodels = [\"m\"]\ndefault_model = \"m\"\n",
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, api::router(AppState::new(config))).await.unwrap() });
    url
}

#[tokio::test]
async fn workers_need_the_token_and_status_does_not() {
    let url = serve().await;
    let http = reqwest::Client::new();
    let body = json!({ "worker_id": "w", "agent": "a", "orchestrator_url": "http://o", "worker_token": "t" });
    for token in [None, Some("wrong")] {
        let mut start = http.post(format!("{url}/workers")).json(&body);
        let mut stop = http.delete(format!("{url}/workers/w"));
        if let Some(t) = token {
            start = start.bearer_auth(t);
            stop = stop.bearer_auth(t);
        }
        assert_eq!(start.send().await.unwrap().status(), StatusCode::UNAUTHORIZED, "start with {token:?}");
        assert_eq!(stop.send().await.unwrap().status(), StatusCode::UNAUTHORIZED, "stop with {token:?}");
    }
    // The right token gets past auth: an unknown worker is 404, not 401.
    assert_eq!(http.delete(format!("{url}/workers/w")).bearer_auth("provider-secret").send().await.unwrap().status(), StatusCode::NOT_FOUND);
    assert_ne!(http.get(format!("{url}/status")).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
}
