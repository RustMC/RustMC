//! Protocol encryption for online-mode logins: the server RSA key, the
//! AES/CFB8 stream ciphers and the session-server hash.
//!
//! Why: an online-mode server must prove to Mojang's session server that the
//! client it talks to holds the shared secret, and must then encrypt every
//! byte of the connection.
//!
//! Done: 1024-bit key generation, DER public key, PKCS#1 v1.5 decryption,
//! continuous AES-128/CFB8 in both directions, the signed SHA-1 hex digest.
//! Not done: key rotation (one key per server run, as vanilla does).
//!
//! Source: minecraft.wiki `Java Edition protocol/Encryption` — "The server
//! generates a 1024-bit RSA keypair on startup", the public key is "encoded in
//! ASN.1 DER format" as an X.509 `SubjectPublicKeyInfo`, the client encrypts
//! with "PKCS#1 v1.5 padded" RSA, and the server "makes two ciphers, one for
//! encryption and one for decryption, with the key and initial vector (IV)
//! both set to the shared secret"; "the AES cipher is updated continuously,
//! not finished and restarted every packet". Primitives come from the
//! RustCrypto crates `rsa`, `aes`, `cfb8` and `sha1`; nothing here implements
//! a cipher itself.

use std::fmt;

use aes::Aes128;
use aes::cipher::KeyIvInit;
use rsa::PrivateKey;
use rsa::pkcs8::EncodePublicKey;
use sha1::{Digest, Sha1};

/// RSA modulus size the wiki gives for the vanilla server key.
pub const SERVER_KEY_BITS: usize = 1024;
/// Length of the client-chosen AES key ("a random 16-byte (128-bit) shared
/// secret").
pub const SHARED_SECRET_BYTES: usize = 16;
/// Length of the server's verify token. The wiki leaves the length to the
/// server; four random bytes make a forged response a 1-in-2^32 guess for a
/// single handshake, which ends the connection on failure.
pub const VERIFY_TOKEN_BYTES: usize = 4;

/// A failure in the encryption handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// The operating system random source failed.
    Random,
    /// The RSA key could not be generated or encoded.
    Key,
    /// A client value did not decrypt with the server key.
    Decrypt,
    /// The decrypted shared secret is not 16 bytes.
    SecretLength(usize),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Random => f.write_str("the system random source failed"),
            Self::Key => f.write_str("the server RSA key could not be created"),
            Self::Decrypt => f.write_str("a client value did not decrypt"),
            Self::SecretLength(n) => write!(f, "shared secret has {n} bytes, expected 16"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Fills `out` from the operating system's random source.
pub fn random_bytes(out: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::fill(out).map_err(|_| CryptoError::Random)
}

/// The server's RSA key pair, created once per run.
pub struct ServerKey {
    private: PrivateKey,
    public_der: Vec<u8>,
}

impl ServerKey {
    /// Generates a fresh key from the system random source.
    pub fn generate() -> Result<Self, CryptoError> {
        let private = PrivateKey::new(&mut getrandom::SysRng, SERVER_KEY_BITS)
            .map_err(|_| CryptoError::Key)?;
        let public_der = private
            .to_public_key()
            .to_public_key_der()
            .map_err(|_| CryptoError::Key)?
            .into_vec();
        Ok(Self {
            private,
            public_der,
        })
    }

    /// The public key as sent in Encryption Request.
    pub fn public_der(&self) -> &[u8] {
        &self.public_der
    }

    /// Decrypts one PKCS#1 v1.5 block from Encryption Response.
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        self.private
            .pkcs1v15_decrypt(ciphertext)
            .map_err(|_| CryptoError::Decrypt)
    }

    /// Decrypts the shared secret and checks its length.
    pub fn decrypt_secret(
        &self,
        ciphertext: &[u8],
    ) -> Result<[u8; SHARED_SECRET_BYTES], CryptoError> {
        let secret = self.decrypt(ciphertext)?;
        <[u8; SHARED_SECRET_BYTES]>::try_from(secret.as_slice())
            .map_err(|_| CryptoError::SecretLength(secret.len()))
    }

    /// Encrypts with the public key, as a client does; used by tests and the
    /// bot client.
    pub fn encrypt_for_test(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        self.private
            .to_public_key()
            .pkcs1v15_encrypt(plaintext)
            .map_err(|_| CryptoError::Decrypt)
    }
}

impl fmt::Debug for ServerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerKey")
            .field("public_der_bytes", &self.public_der.len())
            .finish_non_exhaustive()
    }
}

/// Encrypts the outgoing byte stream of one connection.
pub struct StreamEncryptor(cfb8::Encryptor<Aes128>);

/// Decrypts the incoming byte stream of one connection.
pub struct StreamDecryptor(cfb8::Decryptor<Aes128>);

impl StreamEncryptor {
    pub fn new(secret: &[u8; SHARED_SECRET_BYTES]) -> Self {
        Self(cfb8::Encryptor::new(secret.into(), secret.into()))
    }

    /// Encrypts `bytes` in place, continuing the stream.
    pub fn apply(&mut self, bytes: &mut [u8]) {
        self.0.encrypt(bytes);
    }
}

impl StreamDecryptor {
    pub fn new(secret: &[u8; SHARED_SECRET_BYTES]) -> Self {
        Self(cfb8::Decryptor::new(secret.into(), secret.into()))
    }

    /// Decrypts `bytes` in place, continuing the stream.
    pub fn apply(&mut self, bytes: &mut [u8]) {
        self.0.decrypt(bytes);
    }
}

/// The session-server hash: SHA-1 over `parts`, printed as a signed
/// two's-complement big-endian integer in lowercase hex without leading zeros
/// (wiki: "treating the sha1 output bytes as one large integer in two's
/// complement and then printing the integer in base 16, placing a minus sign
/// if the interpreted number is negative").
pub fn server_hash(parts: &[&[u8]]) -> String {
    let mut sha = Sha1::new();
    for part in parts {
        sha.update(part);
    }
    let mut digest: [u8; 20] = sha.finalize().into();
    let negative = digest[0] & 0x80 != 0;
    if negative {
        // Two's-complement negation: invert, then add one from the low end.
        let mut carry = true;
        for byte in digest.iter_mut().rev() {
            let (value, overflow) = (!*byte).overflowing_add(u8::from(carry));
            *byte = value;
            carry = overflow;
        }
    }
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let trimmed = hex.trim_start_matches('0');
    let digits = if trimmed.is_empty() { "0" } else { trimmed };
    if negative {
        format!("-{digits}")
    } else {
        digits.to_owned()
    }
}

/// A fresh random shared secret for tests, assembled from two random
/// integers.
#[cfg(test)]
pub(crate) fn test_secret() -> [u8; SHARED_SECRET_BYTES] {
    let high = getrandom::u64().expect("random").to_be_bytes();
    let low = getrandom::u64().expect("random").to_be_bytes();
    let mut secret = high.to_vec();
    secret.extend_from_slice(&low);
    secret.try_into().expect("sixteen bytes")
}

#[cfg(test)]
mod tests;
