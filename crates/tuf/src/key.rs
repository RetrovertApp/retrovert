//! Ed25519 keys: generation, PKCS#8 PEM storage, and the TUF key object.

use std::collections::BTreeMap;

use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::canonical;
use crate::error::{Error, Result};

/// The `keytype` every Retrovert channel uses.
pub const KEY_TYPE: &str = "ed25519";

/// The `scheme` every Retrovert channel uses.
pub const SCHEME: &str = "ed25519";

/// The public half of a signing key, in the shape TUF metadata declares it.
///
/// Additional producer fields are preserved so they remain covered by
/// canonicalization when metadata is verified or reserialized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicKey {
    /// Always [`KEY_TYPE`].
    pub keytype: String,
    /// Always [`SCHEME`].
    pub scheme: String,
    /// The key material.
    pub keyval: KeyVal,
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// The inner `keyval` object of a TUF key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyVal {
    /// Hex-encoded raw 32-byte Ed25519 public key.
    pub public: String,
    /// Additional fields defined by a compatible TUF producer.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl PublicKey {
    /// The TUF key ID: SHA-256 of this key's canonical JSON.
    pub fn key_id(&self) -> Result<String> {
        Ok(hex::encode(Sha256::digest(canonical::to_bytes(self)?)))
    }

    /// The raw 32-byte public key.
    pub fn to_verifying_key(&self) -> Result<VerifyingKey> {
        let decoded = hex::decode(&self.keyval.public)
            .map_err(|e| Error::key("ed25519 public key is not valid hex", e))?;
        let raw: [u8; 32] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| Error::PublicKeyLength(decoded.len()))?;
        VerifyingKey::from_bytes(&raw)
            .map_err(|e| Error::key("ed25519 public key is not a valid curve point", e))
    }
}

/// An Ed25519 signing key held in memory.
#[derive(Debug, Clone)]
pub struct KeyPair {
    inner: SigningKey,
}

impl KeyPair {
    /// Generate a key from the operating system's random source.
    pub fn generate() -> Result<Self> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut()).map_err(Error::Random)?;
        Ok(Self::from_seed(&seed))
    }

    /// Build a key from a fixed 32-byte seed. Tests use this for reproducible
    /// output; production paths use [`KeyPair::generate`].
    #[must_use]
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            inner: SigningKey::from_bytes(seed),
        }
    }

    /// Decode a PKCS#8 PEM private key.
    pub fn from_pkcs8_pem(pem: &str) -> Result<Self> {
        SigningKey::from_pkcs8_pem(pem)
            .map(|inner| Self { inner })
            .map_err(|e| Error::key("could not decode a PKCS#8 PEM private key", e))
    }

    /// Encode as a PKCS#8 PEM private key — the form stored offline and pasted
    /// into a GitHub secret.
    pub fn to_pkcs8_pem(&self) -> Result<Zeroizing<String>> {
        self.inner
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| Error::key("could not encode a PKCS#8 PEM private key", e))
    }

    /// The TUF public key object for this key.
    #[must_use]
    pub fn public(&self) -> PublicKey {
        PublicKey {
            keytype: KEY_TYPE.to_string(),
            scheme: SCHEME.to_string(),
            keyval: KeyVal {
                public: hex::encode(self.inner.verifying_key().to_bytes()),
                extra: BTreeMap::new(),
            },
            extra: BTreeMap::new(),
        }
    }

    /// The TUF key ID of this key's public half.
    pub fn key_id(&self) -> Result<String> {
        self.public().key_id()
    }

    /// Sign `message`, returning the hex-encoded 64-byte signature TUF expects.
    #[must_use]
    pub fn sign_hex(&self, message: &[u8]) -> String {
        hex::encode(self.inner.sign(message).to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier};

    fn key() -> KeyPair {
        KeyPair::from_seed(&[7u8; 32])
    }

    #[test]
    fn key_id_is_sha256_of_canonical_key_object() {
        let public = key().public();
        let expected = hex::encode(Sha256::digest(
            format!(
                r#"{{"keytype":"ed25519","keyval":{{"public":"{}"}},"scheme":"ed25519"}}"#,
                public.keyval.public
            )
            .as_bytes(),
        ));
        assert_eq!(public.key_id().unwrap(), expected);
    }

    #[test]
    fn extra_key_fields_survive_a_round_trip() {
        let public = key().public();
        let json = format!(
            r#"{{"keytype":"ed25519","scheme":"ed25519","keyval":{{"public":"{}","x-keyval":"preserved"}},"keyid_hash_algorithms":["sha256"]}}"#,
            public.keyval.public
        );
        let expected: serde_json::Value = serde_json::from_str(&json).unwrap();
        let parsed: PublicKey = serde_json::from_str(&json).unwrap();

        assert_eq!(serde_json::to_value(parsed).unwrap(), expected);
    }

    #[test]
    fn pkcs8_pem_round_trips() {
        let pem = key().to_pkcs8_pem().unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert_eq!(
            KeyPair::from_pkcs8_pem(&pem).unwrap().public(),
            key().public()
        );
    }

    #[test]
    fn signature_verifies_against_the_declared_public_key() {
        let pair = key();
        let sig = hex::decode(pair.sign_hex(b"payload")).unwrap();
        let sig = Signature::from_slice(&sig).unwrap();
        assert!(
            pair.public()
                .to_verifying_key()
                .unwrap()
                .verify(b"payload", &sig)
                .is_ok()
        );
    }
}
