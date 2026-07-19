//! PKCE (Proof Key for Code Exchange) utilities.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `utils/oauth/pkce.ts`.

use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

fn base64url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A generated PKCE verifier/challenge pair.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// Generate a PKCE code verifier and its S256 challenge.
pub fn generate_pkce() -> Pkce {
    let mut verifier_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut verifier_bytes);
    let verifier = base64url_encode(&verifier_bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = base64url_encode(&hasher.finalize());

    Pkce {
        verifier,
        challenge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_pkce_produces_distinct_values() {
        let a = generate_pkce();
        let b = generate_pkce();
        assert_ne!(a.verifier, b.verifier, "verifiers should be random");
        assert_ne!(a.verifier, a.challenge);
    }

    #[test]
    fn test_pkce_challenge_is_sha256_of_verifier() {
        let pkce = generate_pkce();
        let mut hasher = Sha256::new();
        hasher.update(pkce.verifier.as_bytes());
        let expected = base64url_encode(&hasher.finalize());
        assert_eq!(pkce.challenge, expected);
    }

    #[test]
    fn test_base64url_has_no_padding_or_unsafe_chars() {
        let pkce = generate_pkce();
        for s in [&pkce.verifier, &pkce.challenge] {
            assert!(!s.contains('+'));
            assert!(!s.contains('/'));
            assert!(!s.contains('='));
        }
    }
}
