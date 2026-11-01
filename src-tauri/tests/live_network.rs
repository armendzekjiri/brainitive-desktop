//! Live checks of the instance discovery against real hosts. Not run by default (network):
//! `cargo test --test live_network -- --ignored`.

use brainitive_desktop_lib::api::{ApiClient, ApiError};
use brainitive_desktop_lib::server::ServerOrigin;

async fn check(input: &str) -> Result<String, ApiError> {
    let origin = ServerOrigin::parse(input).expect("valid origin");
    ApiClient::new()
        .instance(&origin)
        .await
        .map(|(info, origin)| format!("{} @ {origin}", info.name))
}

#[tokio::test]
#[ignore]
async fn cloud_answers_or_is_recognised_as_an_older_brainitive() {
    // before v1.49.0 is deployed: 404 + our web manifest → ServerTooOld; afterwards: the info
    match check("www.brainitive.app").await {
        Ok(found) => println!("cloud: {found}"),
        Err(e) => assert_eq!(e, ApiError::ServerTooOld, "{e}"),
    }
}

#[tokio::test]
#[ignore]
async fn redirects_via_plain_http_are_refused() {
    // 2026-11-01: https://brainitive.app 301s to *http*://www.brainitive.app — an http hop is
    // never followed (it could be rewritten on the network). Once nginx redirects straight to
    // https://www, this check finds the Cloud instance (or ServerTooOld before v1.49.0).
    match check("brainitive.app").await {
        Ok(found) => assert!(found.ends_with("https://www.brainitive.app"), "{found}"),
        Err(ApiError::BadRedirect(_)) | Err(ApiError::ServerTooOld) => {}
        Err(e) => panic!("{e:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn another_site_is_not_brainitive() {
    assert_eq!(check("example.com").await, Err(ApiError::NotBrainitive));
}

#[tokio::test]
#[ignore]
async fn certificate_errors_are_reported_never_trusted() {
    for host in ["self-signed.badssl.com", "expired.badssl.com", "wrong.host.badssl.com"] {
        match check(host).await {
            Err(ApiError::Tls(reason)) => println!("{host}: {reason}"),
            other => panic!("{host}: expected a TLS error, got {other:?}"),
        }
    }
}
