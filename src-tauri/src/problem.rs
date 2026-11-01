//! Why a server can't be used, in plain language (BRAI-367). Pure: classifies an [`ApiError`]
//! (or the text of a transport error) into a [`ProblemKind`] with a message and a suggested fix.
//! Nothing here is ever "retry and hope": certificate and not-a-Brainitive-server problems are
//! never retried automatically, because only the user or the administrator can fix them.

use serde::Serialize;

use crate::api::{ApiError, TlsReason};
use crate::session::SessionError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    /// This computer has no network.
    Offline,
    /// The name of the server can't be resolved.
    Dns,
    /// The host answered that nothing listens on that port.
    Refused,
    Timeout,
    TlsUntrusted,
    TlsExpired,
    TlsWrongHost,
    TlsOther,
    /// HTTP 5xx (or 408 / 429): the server is up but failing or busy.
    Server,
    NotBrainitive,
    /// The server redirects to an address the app does not follow.
    Moved,
    /// A Brainitive server older than the App API.
    ServerTooOld,
    /// Everything else (connection reset, an unexpected answer, …).
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub kind: ProblemKind,
    pub title: String,
    pub message: String,
    /// What the user can do about it.
    pub fix: String,
    /// Worth trying again by itself (backoff, network back). Certificate problems and wrong
    /// addresses are not: they need a person.
    pub retryable: bool,
    /// Unix time (ms) of the next automatic attempt; filled in by the reconnect loop.
    pub retry_at_ms: Option<u64>,
    /// Automatic attempts made so far.
    pub attempts: u32,
}

impl Problem {
    fn new(kind: ProblemKind, title: String, message: &str, fix: &str, retryable: bool) -> Self {
        Self {
            kind,
            title,
            message: message.to_string(),
            fix: fix.to_string(),
            retryable,
            retry_at_ms: None,
            attempts: 0,
        }
    }

    /// `host`: what to call the server in the sentence ("brainitive.example.com").
    pub fn from_api(error: &ApiError, host: &str) -> Self {
        use ProblemKind as K;
        match error {
            ApiError::Network(chain) => Self::from_network(chain, host),
            ApiError::Timeout => Self::timeout(host),
            ApiError::Tls(reason) => Self::tls(*reason, host),
            ApiError::NotBrainitive | ApiError::InvalidResponse => Self::new(
                K::NotBrainitive,
                format!("{host} is not a Brainitive server"),
                "The address answered, but not like a Brainitive server.",
                "Check the address. Brainitive Cloud is www.brainitive.app; for a self-hosted server use the address you open Brainitive at in your browser.",
                false,
            ),
            ApiError::BadRedirect(_) => Self::new(
                K::Moved,
                format!("{host} redirects somewhere the app won't go"),
                "The server forwards to an address that is not allowed (the app only follows redirects to other https servers).",
                "Use the server's final address, or ask your administrator to fix the redirect.",
                false,
            ),
            ApiError::ServerTooOld => Self::new(
                K::ServerTooOld,
                format!("{host} is too old for the desktop app"),
                "This Brainitive server does not offer the App API the desktop app needs (Brainitive 1.49 or later).",
                "Ask your administrator to update the server, or use Brainitive in a browser meanwhile.",
                false,
            ),
            ApiError::Api { status, .. } if *status >= 500 || *status == 408 || *status == 429 => Self::new(
                K::Server,
                format!("{host} has a problem (HTTP {status})"),
                if *status == 429 {
                    "The server is busy and asks the app to slow down."
                } else {
                    "The server is running but could not handle the request. This is on the server's side, not on your computer."
                },
                "Try again in a few minutes. If it keeps happening, tell the administrator of the server.",
                true,
            ),
            ApiError::Api { status, .. } => Self::new(
                K::Other,
                format!("{host} answered unexpectedly (HTTP {status})"),
                "The server sent an answer the app does not understand.",
                "Check that the address is right and that the server is up to date.",
                false,
            ),
            ApiError::Unauthorized { .. } => Self::new(
                K::Other,
                format!("{host} did not accept the sign-in"),
                "The server answered with an authorization error.",
                "Sign in again.",
                false,
            ),
        }
    }

    pub fn from_session(error: &SessionError, host: &str) -> Self {
        match error {
            SessionError::Offline(_) => Self::from_network("connection interrupted", host),
            SessionError::Keychain(m) => Self::new(
                ProblemKind::Other,
                "The sign-in could not be read".to_string(),
                m,
                "Unlock your keychain or credential store and retry, or sign in again.",
                false,
            ),
            other => Self::new(
                ProblemKind::Other,
                format!("{host} could not be used right now"),
                &other.to_string(),
                "Try again in a moment.",
                true,
            ),
        }
    }

    /// A transport error that is not TLS: told apart by what the OS / resolver said.
    pub fn from_network(chain: &str, host: &str) -> Self {
        use ProblemKind as K;
        match classify_network(chain) {
            K::Offline => Self::new(
                K::Offline,
                "No network connection".to_string(),
                "This computer does not seem to be connected to a network.",
                "Check your Wi-Fi or cable (and the VPN, if you need one). Brainitive reconnects by itself as soon as the network is back.",
                true,
            ),
            K::Dns => Self::new(
                K::Dns,
                format!("The address {host} can't be found"),
                "The name could not be resolved. Either the address is mistyped, or this computer has no working internet / DNS right now.",
                "Check the address for typos. On a company network you may need to connect to the VPN first. Brainitive keeps trying in the background.",
                true,
            ),
            K::Refused => Self::new(
                K::Refused,
                format!("{host} refused the connection"),
                "The computer was found, but nothing accepts connections at this address and port.",
                "Check the port and that the server is running. For a self-hosted server, ask your administrator.",
                true,
            ),
            K::Timeout => Self::timeout(host),
            _ => Self::new(
                K::Other,
                format!("{host} can't be reached"),
                "The connection failed or was interrupted.",
                "Check your connection and the address, then retry.",
                true,
            ),
        }
    }

    fn timeout(host: &str) -> Self {
        Self::new(
            ProblemKind::Timeout,
            format!("{host} did not answer in time"),
            "The server did not respond. It may be down, very slow, or a firewall or proxy is dropping the connection.",
            "Retry in a moment. If you are on a VPN or a company network, check that it allows this address.",
            true,
        )
    }

    fn tls(reason: TlsReason, host: &str) -> Self {
        use ProblemKind as K;
        let never = "Brainitive never skips this check, so your data stays protected.";
        match reason {
            TlsReason::Untrusted => Self::new(
                K::TlsUntrusted,
                format!("{host} does not use a trusted certificate"),
                &format!("Its certificate is self-signed or from an authority this computer does not trust. {never}"),
                "Ask your administrator for a certificate from a public authority (for example Let's Encrypt), or install your organisation's root certificate in the system's trust store, then retry.",
                false,
            ),
            TlsReason::Expired => Self::new(
                K::TlsExpired,
                format!("The certificate of {host} has expired"),
                &format!("The server's certificate is out of date. {never}"),
                "The administrator has to renew the certificate. Also check that this computer's date and time are correct.",
                false,
            ),
            TlsReason::WrongHost => Self::new(
                K::TlsWrongHost,
                format!("The certificate does not match {host}"),
                &format!("The server presented a certificate for a different address. {never}"),
                "Check that you typed the right address, or ask your administrator to install a certificate for this one.",
                false,
            ),
            TlsReason::Revoked | TlsReason::Other => Self::new(
                K::TlsOther,
                format!("The secure connection to {host} failed"),
                &format!("The encrypted connection could not be set up ({reason}). {never}"),
                "Check the server's HTTPS setup, a proxy or antivirus that inspects connections, and this computer's date and time.",
                false,
            ),
        }
    }

    /// Whether the reconnect loop probes the network for this problem (a local name lookup every
    /// few seconds) instead of only waiting for the next backoff step.
    pub fn probes_network(&self) -> bool {
        matches!(self.kind, ProblemKind::Offline | ProblemKind::Dns)
    }
}

/// What the OS / resolver said, from the full error chain (lower-cased, several platforms).
pub fn classify_network(chain: &str) -> ProblemKind {
    let c = chain.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| c.contains(n));
    if has(&["timed out", "timeout", "os error 60)", "os error 110)", "os error 10060)"]) {
        ProblemKind::Timeout
    } else if has(&[
        "dns error",
        "failed to lookup address",
        "nodename nor servname",
        "name or service not known",
        "no such host",
        "temporary failure in name resolution",
        "no address associated",
        "getaddrinfo",
        "os error 11001)",
    ]) {
        ProblemKind::Dns
    } else if has(&[
        "network is unreachable",
        "network is down",
        "os error 50)",
        "os error 51)",
        "os error 101)",
        "os error 10050)",
        "os error 10051)",
        "no route to host",
    ]) {
        ProblemKind::Offline
    } else if has(&["connection refused", "os error 61)", "os error 111)", "os error 10061)", "actively refused"]) {
        ProblemKind::Refused
    } else {
        ProblemKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "brain.example.com";

    fn kind(e: ApiError) -> ProblemKind {
        Problem::from_api(&e, HOST).kind
    }

    #[test]
    fn dns_failures_on_every_platform() {
        for chain in [
            "error sending request for url (https://x/api/v1/instance): client error (Connect): dns error: failed to lookup address information: nodename nor servname provided, or not known",
            "dns error: failed to lookup address information: Name or service not known",
            "dns error: No such host is known. (os error 11001)",
            "Temporary failure in name resolution",
        ] {
            assert_eq!(classify_network(chain), ProblemKind::Dns, "{chain}");
            assert_eq!(kind(ApiError::Network(chain.into())), ProblemKind::Dns);
        }
    }

    #[test]
    fn refused_timeout_and_offline() {
        assert_eq!(
            kind(ApiError::Network("tcp connect error: Connection refused (os error 61)".into())),
            ProblemKind::Refused
        );
        assert_eq!(
            kind(ApiError::Network("tcp connect error: No connection could be made because the target machine actively refused it. (os error 10061)".into())),
            ProblemKind::Refused
        );
        assert_eq!(kind(ApiError::Timeout), ProblemKind::Timeout);
        assert_eq!(
            kind(ApiError::Network("tcp connect error: Connection timed out (os error 60)".into())),
            ProblemKind::Timeout
        );
        assert_eq!(
            kind(ApiError::Network("tcp connect error: Network is unreachable (os error 51)".into())),
            ProblemKind::Offline
        );
        assert_eq!(kind(ApiError::Network("connection reset by peer".into())), ProblemKind::Other);
    }

    #[test]
    fn tls_problems_are_told_apart() {
        assert_eq!(kind(ApiError::Tls(TlsReason::Untrusted)), ProblemKind::TlsUntrusted);
        assert_eq!(kind(ApiError::Tls(TlsReason::Expired)), ProblemKind::TlsExpired);
        assert_eq!(kind(ApiError::Tls(TlsReason::WrongHost)), ProblemKind::TlsWrongHost);
        assert_eq!(kind(ApiError::Tls(TlsReason::Revoked)), ProblemKind::TlsOther);
        assert_eq!(kind(ApiError::Tls(TlsReason::Other)), ProblemKind::TlsOther);
        let p = Problem::from_api(&ApiError::Tls(TlsReason::Untrusted), HOST);
        assert!(p.title.contains(HOST));
        assert!(p.message.contains("never skips"));
        assert!(p.fix.contains("certificate"));
    }

    #[test]
    fn http_5xx_and_not_brainitive() {
        let e = |status| ApiError::Api {
            status,
            code: String::new(),
            message: "x".into(),
        };
        for status in [500, 502, 503, 504, 408, 429] {
            let p = Problem::from_api(&e(status), HOST);
            assert_eq!(p.kind, ProblemKind::Server, "{status}");
            assert!(p.retryable);
        }
        assert!(Problem::from_api(&e(503), HOST).title.contains("503"));
        assert_eq!(kind(e(418)), ProblemKind::Other);
        assert_eq!(kind(ApiError::NotBrainitive), ProblemKind::NotBrainitive);
        assert_eq!(kind(ApiError::InvalidResponse), ProblemKind::NotBrainitive);
        assert_eq!(kind(ApiError::ServerTooOld), ProblemKind::ServerTooOld);
        assert_eq!(kind(ApiError::BadRedirect("x".into())), ProblemKind::Moved);
    }

    #[test]
    fn only_transient_problems_retry_by_themselves() {
        let retry = |e: ApiError| Problem::from_api(&e, HOST).retryable;
        assert!(retry(ApiError::Timeout));
        assert!(retry(ApiError::Network("dns error".into())));
        assert!(retry(ApiError::Network("connection refused".into())));
        for tls in [TlsReason::Untrusted, TlsReason::Expired, TlsReason::WrongHost, TlsReason::Other] {
            assert!(!retry(ApiError::Tls(tls)));
        }
        assert!(!retry(ApiError::NotBrainitive));
        assert!(!retry(ApiError::ServerTooOld));
        assert!(!retry(ApiError::BadRedirect("x".into())));
    }

    #[test]
    fn every_problem_has_a_message_and_a_fix() {
        let all = [
            ApiError::Timeout,
            ApiError::NotBrainitive,
            ApiError::ServerTooOld,
            ApiError::InvalidResponse,
            ApiError::BadRedirect("x".into()),
            ApiError::Tls(TlsReason::Expired),
            ApiError::Network("dns error".into()),
            ApiError::Network("network is unreachable".into()),
            ApiError::Network("refused".into()),
            ApiError::Network("connection refused".into()),
            ApiError::Unauthorized {
                code: "x".into(),
                message: "x".into(),
            },
        ];
        for e in all {
            let p = Problem::from_api(&e, HOST);
            assert!(!p.title.is_empty() && !p.message.is_empty() && !p.fix.is_empty(), "{e:?}");
        }
    }

    #[test]
    fn network_probe_only_for_dns_and_offline() {
        assert!(Problem::from_network("dns error", HOST).probes_network());
        assert!(Problem::from_network("network is unreachable", HOST).probes_network());
        assert!(!Problem::from_network("connection refused", HOST).probes_network());
        assert!(!Problem::from_api(&ApiError::Timeout, HOST).probes_network());
    }

    #[test]
    fn session_errors() {
        let p = Problem::from_session(&SessionError::Offline("down".into()), HOST);
        assert!(p.retryable);
        let p = Problem::from_session(&SessionError::Keychain("locked".into()), HOST);
        assert!(!p.retryable);
        assert_eq!(p.message, "locked");
    }
}
