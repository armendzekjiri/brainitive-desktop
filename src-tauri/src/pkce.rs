//! PKCE (RFC 7636, S256) and the `state` of the native sign-in. Pure, unit tested.
//!
//! The server side is `AppAuthCodes::challengeFor` in the app repo:
//! `base64url(sha256(verifier))` without padding.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// One sign-in attempt: the verifier stays in memory, the challenge and state go to the browser.
#[derive(Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl std::fmt::Debug for PkcePair {
    // never print the verifier
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PkcePair")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

impl PkcePair {
    pub fn generate() -> Self {
        let verifier = random_token(48); // 64 chars, inside the 43–128 range of RFC 7636
        let challenge = challenge_for(&verifier);
        let state = random_token(32);
        Self {
            verifier,
            challenge,
            state,
        }
    }
}

/// `n` random bytes from the OS-seeded CSPRNG, base64url without padding.
pub fn random_token(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::fill(&mut bytes[..]);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Compares the `state` of the callback with the expected one in constant time.
pub fn state_matches(expected: &str, received: &str) -> bool {
    let (a, b) = (expected.as_bytes(), received.as_bytes());
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_appendix_b_vector() {
        // https://www.rfc-editor.org/rfc/rfc7636#appendix-B
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_is_valid_and_random() {
        let a = PkcePair::generate();
        let b = PkcePair::generate();
        assert!((43..=128).contains(&a.verifier.len()));
        assert!(a
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~')));
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.state, b.state);
        assert_eq!(a.challenge, challenge_for(&a.verifier));
        assert_eq!(a.challenge.len(), 43);
        assert!(!format!("{a:?}").contains(&a.verifier));
    }

    #[test]
    fn state_check() {
        let p = PkcePair::generate();
        assert!(state_matches(&p.state, &p.state.clone()));
        assert!(!state_matches(&p.state, &PkcePair::generate().state));
        assert!(!state_matches(&p.state, ""));
        assert!(!state_matches("", ""));
        assert!(!state_matches(&p.state, &p.state[1..]));
    }
}
