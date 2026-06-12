//! Symmetric encryption for the Secret store.
//!
//! AES-256-GCM with a fresh random 96-bit nonce per value. The stored blob is
//! `nonce ‖ ciphertext+tag`. The master key comes from the `COVEFLOW_SECRET_KEY`
//! env var (base64-encoded 32 bytes); generate one with `openssl rand -base64 32`.
//!
//! Lives in `coveflow-types` so the API (encrypt on write) and the worker
//! (decrypt on inject) share one implementation and cannot drift.
//!
//! SECURITY: never log or format plaintext or the key. `CryptoError` is
//! deliberately opaque — it carries no plaintext, key bytes, or nonce.

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("secret key must decode to {KEY_LEN} bytes; generate with `openssl rand -base64 32`")]
    KeyLength,
    #[error("secret key is not valid base64")]
    KeyEncoding,
    #[error("encrypted blob is malformed")]
    Malformed,
    #[error("decryption failed (wrong key or tampered data)")]
    Decrypt,
    #[error("encryption failed")]
    Encrypt,
}

/// AES-256 master key. `Clone` is cheap (32 bytes); `Debug` is redacted so the
/// key never leaks into logs even when it is nested in another struct's derive.
#[derive(Clone)]
pub struct SecretKey([u8; KEY_LEN]);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(***)")
    }
}

impl SecretKey {
    /// Parse from a base64-encoded 32-byte value (the `COVEFLOW_SECRET_KEY` env).
    pub fn from_base64(s: &str) -> Result<Self, CryptoError> {
        let bytes = B64.decode(s.trim()).map_err(|_| CryptoError::KeyEncoding)?;
        let arr: [u8; KEY_LEN] = bytes.try_into().map_err(|_| CryptoError::KeyLength)?;
        Ok(Self(arr))
    }

    fn cipher(&self) -> Aes256Gcm {
        // The 32-byte array is exactly an AES-256 key, so this is infallible.
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.0))
    }
}

/// Encrypt `plaintext`, returning `nonce ‖ ciphertext+tag`.
pub fn encrypt(key: &SecretKey, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = key.cipher();
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| CryptoError::Encrypt)?;

    let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(nonce.as_slice());
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

/// Decrypt a `nonce ‖ ciphertext+tag` blob. Fails on a wrong key or tampering.
pub fn decrypt(key: &SecretKey, blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if blob.len() < NONCE_LEN {
        return Err(CryptoError::Malformed);
    }
    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    key.cipher()
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A deterministic, valid 32-byte key for tests (base64 of 32 0x01 bytes).
    fn test_key() -> SecretKey {
        SecretKey::from_base64(&B64.encode([1u8; KEY_LEN])).unwrap()
    }

    #[test]
    fn round_trips() {
        let key = test_key();
        let blob = encrypt(&key, b"hunter2").unwrap();
        assert_eq!(decrypt(&key, &blob).unwrap(), b"hunter2");
    }

    #[test]
    fn ciphertext_is_not_plaintext_and_nonce_randomized() {
        let key = test_key();
        let a = encrypt(&key, b"same").unwrap();
        let b = encrypt(&key, b"same").unwrap();
        // Random nonce → two encryptions of the same plaintext differ.
        assert_ne!(a, b);
        // The plaintext does not appear verbatim in the blob.
        assert!(!a.windows(4).any(|w| w == b"same"));
    }

    #[test]
    fn wrong_key_fails() {
        let blob = encrypt(&test_key(), b"secret").unwrap();
        let other = SecretKey::from_base64(&B64.encode([2u8; KEY_LEN])).unwrap();
        assert!(matches!(decrypt(&other, &blob), Err(CryptoError::Decrypt)));
    }

    #[test]
    fn tampered_tag_fails() {
        let key = test_key();
        let mut blob = encrypt(&key, b"secret").unwrap();
        // Flip a bit in the last byte (part of the GCM tag).
        *blob.last_mut().unwrap() ^= 0x01;
        assert!(matches!(decrypt(&key, &blob), Err(CryptoError::Decrypt)));
    }

    #[test]
    fn truncated_blob_is_malformed() {
        let key = test_key();
        assert!(matches!(
            decrypt(&key, &[0u8; 4]),
            Err(CryptoError::Malformed)
        ));
    }

    #[test]
    fn rejects_bad_base64_and_wrong_length() {
        assert!(matches!(
            SecretKey::from_base64("not valid base64!!!"),
            Err(CryptoError::KeyEncoding)
        ));
        // Valid base64 but only 16 bytes.
        assert!(matches!(
            SecretKey::from_base64(&B64.encode([0u8; 16])),
            Err(CryptoError::KeyLength)
        ));
    }
}
