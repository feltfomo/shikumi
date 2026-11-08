//! nix's base-32 encoding, which store path hash parts and printed hashes use.
//!
//! it isn't RFC 4648: the alphabet drops `e`, `o`, `u` and `t`, and the digits are read from the
//! last bit of the input backwards. both follow `printHash32` in CppNix 2.35.3
//! `src/libutil/hash.cc`, and Lix 2.95.4 encodes the same way.

pub(crate) const ALPHABET: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

pub(crate) fn encoded_len(bytes: usize) -> usize {
    (bytes * 8).div_ceil(5)
}

pub(crate) fn encode(bytes: &[u8]) -> String {
    let len = encoded_len(bytes.len());
    let mut out = String::with_capacity(len);
    for n in (0..len).rev() {
        let bit = n * 5;
        let i = bit / 8;
        let j = bit % 8;
        // the digit's five bits can straddle two bytes; u16 keeps the shift of the next byte in range.
        let low = u16::from(bytes[i]) >> j;
        let high = bytes.get(i + 1).map_or(0, |&b| u16::from(b) << (8 - j));
        out.push(char::from(ALPHABET[usize::from((low | high) & 0x1f)]));
    }
    out
}

pub(crate) fn is_digit(c: u8) -> bool {
    ALPHABET.contains(&c)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `nix-hash --type sha256 --to-base32` of sha256("") on Lix 2.95.4. changing this means the
    // digit order or alphabet moved, and every store path Shikumi computes moves with it.
    #[test]
    fn the_empty_digest_encodes_as_nix_prints_it() {
        let digest = crate::Sha256::digest(b"");
        assert_eq!(
            encode(digest.as_bytes()),
            "0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73"
        );
    }
}
