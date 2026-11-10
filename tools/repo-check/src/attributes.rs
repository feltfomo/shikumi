//! the lint-silencing check: no tracked Rust file carries an `allow` or `expect` attribute, outer or
//! inner, on its own or inside `cfg_attr`, since docs/reference/rust-style.md has a finding fixed
//! rather than silenced. a workspace lint can't hold that: Clippy's `allow_attributes` refuses
//! `allow` alone, and a lint set to `forbid` refuses both only for that one lint, so this reads the
//! source itself: it steps over comments and string and character literals, where the words are
//! only text, and reads each attribute's path.

use crate::{Finding, Position};

/// the attributes that silence a lint.
const SILENCING: [&str; 2] = ["allow", "expect"];

pub(crate) fn applies(path: &str) -> bool {
    path.ends_with(".rs")
}

/// every attribute in `contents` that silences a lint, at its `#`. the scan is written here rather
/// than taken from a Rust parser such as `syn`, which would build the whole syntax tree of every file
/// to answer what stepping over comments and literals and reading one path answers.
pub(crate) fn check(path: &str, contents: &[u8]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut at = 0;
    while at < contents.len() {
        at = match contents[at] {
            b'/' if contents.get(at + 1) == Some(&b'/') => line_end(contents, at),
            b'/' if contents.get(at + 1) == Some(&b'*') => block_end(contents, at),
            b'"' => string_end(contents, at + 1),
            b'r' | b'b' if !ident_byte(at.checked_sub(1).map(|before| contents[before])) => {
                literal_end(contents, at).unwrap_or(at + 1)
            }
            b'\'' => character_end(contents, at),
            b'#' => {
                if let Some(name) = silencing(contents, at) {
                    findings.push(Finding::at(
                        path,
                        position(contents, at),
                        format!(
                            "an `{name}` attribute silences a lint; fix the finding instead, as docs/reference/rust-style.md says"
                        ),
                    ));
                }
                at + 1
            }
            _ => at + 1,
        };
    }
    findings
}

fn ident_byte(byte: Option<u8>) -> bool {
    byte.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn line_end(contents: &[u8], from: usize) -> usize {
    contents[from..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(contents.len(), |end| from + end)
}

/// past a block comment starting at `from`, nested ones included, as Rust nests them.
fn block_end(contents: &[u8], from: usize) -> usize {
    let mut depth = 0;
    let mut at = from;
    while at < contents.len() {
        match (contents[at], contents.get(at + 1)) {
            (b'/', Some(b'*')) => {
                depth += 1;
                at += 2;
            }
            (b'*', Some(b'/')) => {
                depth -= 1;
                at += 2;
                if depth == 0 {
                    return at;
                }
            }
            _ => at += 1,
        }
    }
    at
}

/// past the closing quote of a string whose body starts at `from`, escapes included.
fn string_end(contents: &[u8], from: usize) -> usize {
    let mut at = from;
    while at < contents.len() {
        match contents[at] {
            b'\\' => at += 2,
            b'"' => return at + 1,
            _ => at += 1,
        }
    }
    at
}

/// past a byte string, a raw string or a raw byte string starting at `from`, or none when what's
/// there is an identifier.
fn literal_end(contents: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    if contents[at] == b'b' {
        at += 1;
        match contents.get(at) {
            Some(b'"') => return Some(string_end(contents, at + 1)),
            Some(b'\'') => return Some(character_end(contents, at)),
            Some(b'r') => {}
            _ => return None,
        }
    }
    at += 1;
    let hashes = contents[at..]
        .iter()
        .take_while(|&&byte| byte == b'#')
        .count();
    at += hashes;
    if contents.get(at) != Some(&b'"') {
        return None;
    }
    let mut closing = vec![b'"'];
    closing.extend(std::iter::repeat_n(b'#', hashes));
    Some(
        contents[at + 1..]
            .windows(closing.len())
            .position(|window| window == closing.as_slice())
            .map_or(contents.len(), |end| at + 1 + end + closing.len()),
    )
}

/// past a character literal starting at `from`, or just past the quote of a lifetime or label.
fn character_end(contents: &[u8], from: usize) -> usize {
    match contents.get(from + 1) {
        Some(b'\\') => contents[from + 2..]
            .iter()
            .skip(1)
            .position(|&byte| byte == b'\'')
            .map_or(contents.len(), |end| from + 4 + end),
        Some(&first) => {
            // a character is one UTF-8 sequence, whose length its first byte gives.
            let length = match first {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                _ => 4,
            };
            if contents.get(from + 1 + length) == Some(&b'\'') {
                from + 2 + length
            } else {
                from + 1
            }
        }
        None => from + 1,
    }
}

/// the silencing attribute whose `#` is at `at`: `allow` or `expect` as the attribute's path, or
/// inside a `cfg_attr`'s attributes.
fn silencing(contents: &[u8], at: usize) -> Option<&'static str> {
    let mut next = skip_space(contents, at + 1);
    if contents.get(next) == Some(&b'!') {
        next = skip_space(contents, next + 1);
    }
    if contents.get(next) != Some(&b'[') {
        return None;
    }
    let (path, after) = ident(contents, skip_space(contents, next + 1));
    if let Some(name) = SILENCING.into_iter().find(|name| name.as_bytes() == path) {
        return Some(name);
    }
    if path != b"cfg_attr" {
        return None;
    }
    // the attributes `cfg_attr` applies follow its condition, up to the bracket that closes it.
    let mut depth = 1;
    let mut at = after;
    while at < contents.len() && depth > 0 {
        match contents[at] {
            b'[' | b'(' => depth += 1,
            b']' | b')' => depth -= 1,
            byte if byte.is_ascii_alphabetic() && !ident_byte(Some(contents[at - 1])) => {
                let (word, end) = ident(contents, at);
                let opens = contents.get(skip_space(contents, end)) == Some(&b'(');
                if let Some(name) = SILENCING.into_iter().find(|name| name.as_bytes() == word)
                    && opens
                {
                    return Some(name);
                }
                at = end;
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    None
}

fn skip_space(contents: &[u8], from: usize) -> usize {
    from + contents[from.min(contents.len())..]
        .iter()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count()
}

/// the identifier starting at `from`, and where it ends.
fn ident(contents: &[u8], from: usize) -> (&[u8], usize) {
    let from = from.min(contents.len());
    let length = contents[from..]
        .iter()
        .take_while(|&&byte| ident_byte(Some(byte)))
        .count();
    (&contents[from..from + length], from + length)
}

fn position(contents: &[u8], at: usize) -> Position {
    let before = &contents[..at];
    let line = before.iter().filter(|&&byte| byte == b'\n').count() + 1;
    let start = before
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |newline| newline + 1);
    Position {
        line,
        column: at - start + 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(source: &str) -> Vec<(usize, usize, String)> {
        check("a.rs", source.as_bytes())
            .into_iter()
            .map(|finding| {
                let position = finding.position().unwrap();
                (position.line, position.column, finding.message().to_owned())
            })
            .collect()
    }

    // every way to write a silencing attribute is found, at its `#`: outer and inner, spaced out,
    // with a reason, and inside a `cfg_attr`.
    #[test]
    fn a_planted_silencing_attribute_is_found() {
        let source = "#[allow(dead_code)]\nfn a() {}\n\
                      mod b {\n    #![expect(unused)]\n}\n\
                      # [ allow ( clippy::todo, reason = \"x\" ) ]\nfn c() {}\n\
                      #[cfg_attr(test, allow(dead_code))]\nfn d() {}\n\
                      #[cfg_attr(not(test), derive(Debug), expect(unused))]\nstruct E;\n";
        let positions: Vec<(usize, usize)> = found(source)
            .into_iter()
            .map(|(line, column, _)| (line, column))
            .collect();
        assert_eq!(positions, [(1, 1), (4, 5), (6, 1), (8, 1), (10, 1)]);
        assert!(
            found("#[expect(dead_code)]\nfn a() {}\n")[0]
                .2
                .contains("`expect`")
        );
    }

    // the words in comments, strings and characters are text, and an attribute whose path only
    // starts with one of the names is another attribute.
    #[test]
    fn text_and_other_attributes_are_not_found() {
        let source = "// #[allow(dead_code)]\n\
                      /* outer /* #[allow(x)] */ #[expect(x)] */\n\
                      const A: &str = \"#[allow(x)] \\\" #[expect(x)]\";\n\
                      const B: &str = r#\"#[allow(x)] \" #[expect(x)]\"#;\n\
                      const C: &[u8] = br\"#[allow(x)]\";\n\
                      const D: char = '#';\n\
                      const E: char = '\\'';\n\
                      fn f<'a>(x: &'a str) -> &'a str { x }\n\
                      #[allowed(x)]\n#[cfg(test)]\n#[cfg_attr(test, derive(Debug))]\n\
                      #[derive(Debug)]\nstruct G;\n";
        assert_eq!(found(source), []);
    }

    #[test]
    fn only_rust_files_are_read() {
        assert!(applies("crates/a/src/lib.rs"));
        assert!(!applies("docs/reference/rust-style.md"));
    }
}
