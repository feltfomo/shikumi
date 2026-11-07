//! the sha256 digest and the forms Nix prints it in: lowercase hex, nix base-32 and SRI.

use std::fmt;

use sha2::Digest;

use crate::base32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256([u8; 32]);

impl Sha256 {
    pub fn digest(bytes: &[u8]) -> Self {
        Self(sha2::Sha256::digest(bytes).into())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        let mut out = [0u8; 64];
        self.write_hex(&mut out).to_owned()
    }

    /// lowercase hex into a caller's buffer, for fingerprints hashed without building a string.
    pub(crate) fn write_hex<'a>(&self, out: &'a mut [u8; 64]) -> &'a str {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for (pair, &b) in out.as_chunks_mut::<2>().0.iter_mut().zip(&self.0) {
            *pair = [DIGITS[usize::from(b >> 4)], DIGITS[usize::from(b & 0xf)]];
        }
        std::str::from_utf8(out).expect("hex digits are ASCII")
    }

    pub fn to_base32(&self) -> String {
        base32::encode(&self.0)
    }

    /// the `sha256-<base64>` form CppNix's derivation JSON uses for a fixed output's hash.
    pub fn to_sri(&self) -> String {
        format!("sha256-{}", base64(&self.0))
    }

    /// reads the bare lowercase hex Nix prints for a NAR hash in `nix-store --dump-db`.
    pub fn parse_hex(text: &str) -> Result<Self, HashParseError> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return Err(HashParseError(text.to_owned()));
        }
        let mut out = [0u8; 32];
        for (byte, pair) in out.iter_mut().zip(bytes.as_chunks::<2>().0) {
            let high = hex_value(pair[0]).ok_or_else(|| HashParseError(text.to_owned()))?;
            let low = hex_value(pair[1]).ok_or_else(|| HashParseError(text.to_owned()))?;
            *byte = high << 4 | low;
        }
        Ok(Self(out))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashParseError(String);

impl fmt::Display for HashParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}' isn't a sha256 in lowercase hex", self.0)
    }
}

impl std::error::Error for HashParseError {}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// standard base64 with padding. a few lines here rather than a crate, since it's the only use and
/// only ever encodes 32 bytes.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // sha256("") in each form, as `nix hash convert` prints it on Lix 2.95.4. a change here moves
    // every fixed-output hash Shikumi writes into derivation JSON.
    #[test]
    fn the_empty_digest_prints_in_every_form_nix_uses() {
        let digest = Sha256::digest(b"");
        assert_eq!(
            digest.to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            digest.to_sri(),
            "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
    }

    #[test]
    fn hex_parses_back_to_the_same_digest() {
        let digest = Sha256::digest(b"shikumi");
        assert_eq!(Sha256::parse_hex(&digest.to_hex()), Ok(digest));
    }

    #[test]
    fn uppercase_or_short_hex_is_rejected() {
        let upper = Sha256::digest(b"x").to_hex().to_uppercase();
        assert!(Sha256::parse_hex(&upper).is_err());
        assert!(Sha256::parse_hex("abcd").is_err());
    }

    #[test]
    fn base64_pads_partial_groups() {
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
    }
}
