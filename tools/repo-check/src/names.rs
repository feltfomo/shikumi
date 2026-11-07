//! the private-name check. this repository is public and must not name a private consumer or the
//! maintainer's machines, so the check can't hold the names either: it holds a salt and the SHA-256
//! of the salt followed by each lowercased name, and compares tokens of every tracked file against
//! those digests. anyone with a word list can still confirm a guess; any check that runs in a public
//! gate is that oracle. the salt only stops a lookup-table reversal.
//!
//! matching is exact on tokens, not a substring search. a token is a run of ASCII letters, split
//! further where a lowercase letter meets an uppercase one and before the last capital of a capital
//! run that starts a word, so `name.example`, `name_2`, `/srv/name/` and `fooNameBar` are all
//! caught. a name fused into a longer token with no case change or separator is not.

use sha2::{Digest, Sha256};

use crate::{Finding, Position};

const SALT: &str = "64fb0c4f55f0787eb989c037a9024241";

/// add a name with `repo-check digest`, which reads it from stdin and prints its line.
const DIGESTS: &[&str] = &[
    "0a00a7b4b93c4891fe8de630b2263eb82baf0b49d04ece093371d3b8e55d0081",
    "0b36b7eda1fc6f8ea9b88bd51f5b53a4654276b656fe068cf8ab31c3b2a05548",
    "1101589096775ce7bd30e86cbe2ed61ced2a1eb66e40efe622ce4f883fec9ce7",
    "1e7c32a2b586565cc7b4979de2b87ecfdd306ab77bdfac03aac609726e3520b1",
    "597979dce823aa5b4b2e09d42e82ae3d7796e27d4bb15ee3b9660fdea245f79e",
    "931153bdf2566ffe7b2e5082b430dc7575a082cef446c092437109e2adcdca43",
    "a03effe5ebeb855cc1d0d774b65377c5349a752c3eed3cf4ba8669720cca572a",
];

#[derive(Debug, Clone)]
pub(crate) struct Matcher {
    salted: Sha256,
    digests: Vec<[u8; 32]>,
}

impl Matcher {
    pub(crate) fn protected() -> Self {
        let digests = DIGESTS
            .iter()
            .map(|hex| decode_hex(hex).expect("DIGESTS holds 64-digit lowercase hex"))
            .collect();
        Self::new(SALT, digests)
    }

    fn new(salt: &str, mut digests: Vec<[u8; 32]>) -> Self {
        // the lookup is a binary search, and this sort is the one place its order comes from.
        digests.sort_unstable();
        Self {
            salted: Sha256::new_with_prefix(salt.as_bytes()),
            digests,
        }
    }

    fn is_protected(&self, lowercase_token: &[u8]) -> bool {
        self.digests
            .binary_search(&salted_digest(&self.salted, lowercase_token))
            .is_ok()
    }

    /// every protected token in `bytes`, as 1-based line and byte column.
    pub(crate) fn scan(&self, bytes: &[u8]) -> Vec<Position> {
        let mut hits = Vec::new();
        let mut token = Vec::with_capacity(64);
        let mut line = 1;
        let mut line_start = 0;
        let mut i = 0;
        while i < bytes.len() {
            if !bytes[i].is_ascii_alphabetic() {
                if bytes[i] == b'\n' {
                    line += 1;
                    line_start = i + 1;
                }
                i += 1;
                continue;
            }
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() && !is_boundary(bytes, i) {
                i += 1;
            }
            token.clear();
            token.extend(bytes[start..i].iter().map(u8::to_ascii_lowercase));
            if self.is_protected(&token) {
                hits.push(Position {
                    line,
                    column: start - line_start + 1,
                });
            }
        }
        hits
    }
}

/// the line `repo-check digest` prints for a name.
pub(crate) fn digest_line(word: &str) -> String {
    let salted = Sha256::new_with_prefix(SALT.as_bytes());
    hex(&salted_digest(
        &salted,
        word.to_ascii_lowercase().as_bytes(),
    ))
}

fn salted_digest(salted: &Sha256, lowercase_token: &[u8]) -> [u8; 32] {
    salted
        .clone()
        .chain_update(lowercase_token)
        .finalize()
        .into()
}

/// whether a new token starts at `i`, inside a run of letters.
fn is_boundary(bytes: &[u8], i: usize) -> bool {
    let (prev, cur) = (bytes[i - 1], bytes[i]);
    let next_is_lower = bytes.get(i + 1).is_some_and(u8::is_ascii_lowercase);
    (prev.is_ascii_lowercase() && cur.is_ascii_uppercase())
        || (prev.is_ascii_uppercase() && cur.is_ascii_uppercase() && next_is_lower)
}

pub(crate) fn check(matcher: &Matcher, path: &str, contents: &[u8]) -> Vec<Finding> {
    let mut findings = Vec::new();
    if !matcher.scan(path.as_bytes()).is_empty() {
        findings.push(Finding::file(path, "the path contains a protected name"));
    }
    for position in matcher.scan(contents) {
        findings.push(Finding::at(path, position, "contains a protected name"));
    }
    findings
}

fn decode_hex(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (byte, &[high, low]) in out.iter_mut().zip(bytes.as_chunks::<2>().0) {
        *byte = (digit(high)? << 4) | digit(low)?;
    }
    Some(out)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // a decoy under a test-only salt, so the test proves detection without holding a real name.
    fn decoy() -> Matcher {
        let salted = Sha256::new_with_prefix(b"test-salt");
        Matcher::new("test-salt", vec![salted_digest(&salted, b"zorblax")])
    }

    fn scan(matcher: &Matcher, text: &str) -> Vec<Position> {
        matcher.scan(text.as_bytes())
    }

    #[test]
    fn catches_the_name_in_every_token_shape() {
        let matcher = decoy();
        for text in [
            "zorblax",
            "the Zorblax host",
            "ZORBLAX",
            "zorblax.example",
            "my_zorblax_2",
            "zorblax-run --flag",
            "/srv/zorblax/data",
            "fooZorblaxBar",
            "HTTPZorblax",
            "9zorblax9",
        ] {
            assert_eq!(scan(&matcher, text).len(), 1, "missed in {text:?}");
        }
    }

    #[test]
    fn reports_line_and_column() {
        let hits = scan(&decoy(), "first line\n  see zorblax.example\n");
        assert_eq!(hits, [Position { line: 2, column: 7 }]);
    }

    #[test]
    fn leaves_fused_and_unrelated_tokens_alone() {
        let matcher = decoy();
        for text in ["zorblaxing", "unzorblax", "zorbla x", "Zorblaxer", ""] {
            assert!(scan(&matcher, text).is_empty(), "flagged {text:?}");
        }
    }

    #[test]
    fn flags_a_path_and_contents_separately() {
        let findings = check(
            &decoy(),
            "docs/zorblax.md",
            b"ok
zorblax
",
        );
        assert_eq!(findings.len(), 2);
        // the report names the file and position, never the word.
        assert!(findings.iter().all(|f| !f.message().contains("zorblax")));
    }

    #[test]
    fn protected_digests_are_well_formed() {
        assert_eq!(Matcher::protected().digests.len(), DIGESTS.len());
        assert!(scan(&Matcher::protected(), "shikumi build system").is_empty());
    }

    #[test]
    fn digest_line_round_trips_through_the_matcher() {
        let digest = decode_hex(&digest_line("Example")).unwrap();
        let matcher = Matcher::new(SALT, vec![digest]);
        assert_eq!(scan(&matcher, "an example here").len(), 1);
    }
}
