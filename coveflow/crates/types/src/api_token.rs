//! Personal API Token (PAT) string helpers.
//!
//! A PAT is a user-owned bearer credential, shown once at creation and stored
//! two ways: a `sha256` hash (the O(1) auth lookup key) and an encrypted copy
//! (so the UI can reveal it later, see [`crate::crypto`]). This module owns only
//! the token *string* concerns — generation and hashing.
//!
//! SECURITY: never log the token string or its hash.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use sha2::{Digest, Sha256};

/// Prefix on every token so a leaked token is recognizable to secret scanners.
pub const TOKEN_PREFIX: &str = "cf_pat_";

/// Generate a new token: `cf_pat_` + 32 CSPRNG bytes (base64url, no padding).
pub fn generate_token() -> String {
    use aes_gcm::aead::OsRng;
    use aes_gcm::aead::rand_core::RngCore;
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("{TOKEN_PREFIX}{}", B64URL.encode(bytes))
}

/// sha256 hex of the token — the DB lookup key (non-secret, indexable, O(1)).
pub fn token_hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_prefixed_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert!(a.starts_with(TOKEN_PREFIX));
        assert_ne!(a, b, "two tokens must differ");
        // 32 bytes base64url (no pad) = 43 chars, plus the prefix.
        assert_eq!(a.len(), TOKEN_PREFIX.len() + 43);
    }

    #[test]
    fn hash_is_deterministic_64_hex() {
        let t = "cf_pat_abc";
        let h = token_hash(t);
        assert_eq!(h, token_hash(t), "stable");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(
            h,
            token_hash("cf_pat_abd"),
            "different token → different hash"
        );
    }
}
