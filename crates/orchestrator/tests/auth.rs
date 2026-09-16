use std::time::Duration;

use api_client::{ApproveUser, Challenge, Client, Error, Login, Session};
use ic_auth_verifier::{new_basic_identity, BasicIdentity, SignedEnvelope};
use orchestrator::auth::{issue_challenge, CHALLENGE_TTL};

mod common;
use common::TOKEN;

async fn serve() -> (String, tempfile::TempDir) {
    common::serve(include_str!("../../../factory.example.toml")).await
}

/// What the browser does: sign the challenge string the way `@ldclabs/ic-auth`'s `signMessage` does.
fn envelope(identity: &BasicIdentity, challenge: &str) -> String {
    SignedEnvelope::sign_message(identity, &ic_auth_types::deterministic_cbor_into_vec(challenge).unwrap()).unwrap().to_base64()
}

async fn challenge(url: &str) -> String {
    reqwest::get(format!("{url}/auth/challenge")).await.unwrap().json::<Challenge>().await.unwrap().challenge
}

async fn login(url: &str, envelope: String) -> Result<Session, u16> {
    let r = reqwest::Client::new().post(format!("{url}/auth/login")).json(&Login { envelope }).send().await.unwrap();
    if r.status().is_success() { Ok(r.json().await.unwrap()) } else { Err(r.status().as_u16()) }
}

fn status<T: std::fmt::Debug>(r: Result<T, Error>) -> u16 {
    match r {
        Err(Error::Api { status, .. }) => status,
        other => panic!("expected an API error, got {other:?}"),
    }
}

#[tokio::test]
async fn ed25519_identity_logs_in_as_a_pending_user_and_logs_out() {
    let (url, _dir) = serve().await;
    let admin = Client::new(&url, TOKEN);
    let identity = new_basic_identity();
    let principal = ic_auth_verifier::Identity::sender(&identity).unwrap().to_text();

    let s = login(&url, envelope(&identity, &challenge(&url).await)).await.unwrap();
    assert_eq!((s.principal.as_str(), s.status.as_str()), (principal.as_str(), "pending"));
    let me = Client::new(&url, &s.token);
    assert_eq!(me.me().await.unwrap().status, "pending");
    assert_eq!(status(me.list_tickets(&Default::default()).await), 403);
    assert_eq!(admin.list_users().await.unwrap().iter().map(|u| u.principal.as_str()).collect::<Vec<_>>(), [principal.as_str()]);

    // Approval applies to the same session; a second login reports the current status and opens another session.
    admin.approve_user(&principal, &ApproveUser { name: "Ed".into() }).await.unwrap();
    me.list_tickets(&Default::default()).await.unwrap();
    let again = login(&url, envelope(&identity, &challenge(&url).await)).await.unwrap();
    assert_eq!(again.status, "approved");
    assert_ne!(again.token, s.token);

    // Logout ends only the session it was called with; the admin and personal tokens have no session.
    me.logout().await.unwrap();
    assert_eq!(status(me.me().await), 401);
    Client::new(&url, &again.token).me().await.unwrap();
    assert_eq!(status(admin.logout().await), 404);
}

#[tokio::test]
async fn challenges_are_single_use_and_expire() {
    let (url, _dir) = serve().await;
    let identity = new_basic_identity();
    let c = challenge(&url).await;
    login(&url, envelope(&identity, &c)).await.unwrap();
    assert_eq!(login(&url, envelope(&identity, &c)).await.unwrap_err(), 400);
    assert_eq!(login(&url, envelope(&identity, "never-issued")).await.unwrap_err(), 400);
    let expired = issue_challenge(Duration::ZERO);
    assert_eq!(login(&url, envelope(&identity, &expired)).await.unwrap_err(), 400);
    // Sanity: the served challenge lives long enough to be used.
    assert!(CHALLENGE_TTL >= Duration::from_secs(60));
}

#[tokio::test]
async fn bad_signatures_and_malformed_envelopes_are_rejected() {
    let (url, _dir) = serve().await;
    let identity = new_basic_identity();
    let c = challenge(&url).await;
    // Another identity's signature over the same digest, presented under this identity's key.
    let mut forged = SignedEnvelope::from_base64(&envelope(&identity, &c)).unwrap();
    forged.signature = SignedEnvelope::from_base64(&envelope(&new_basic_identity(), &c)).unwrap().signature;
    assert_eq!(login(&url, forged.to_base64()).await.unwrap_err(), 400);
    assert_eq!(login(&url, "not-an-envelope".into()).await.unwrap_err(), 400);
    // The challenge survives failed attempts and still works once.
    login(&url, envelope(&identity, &c)).await.unwrap();
    assert!(Client::new(&url, TOKEN).list_users().await.unwrap().len() == 1);
}
