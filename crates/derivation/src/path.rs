//! store directories, store path names and store paths, with the name rules CppNix and Lix share.

use std::fmt;

use crate::base32;

/// length of a store path's hash part: a 160-bit digest in nix base-32.
pub const HASH_PART_LEN: usize = 32;

/// a store path's hash part as a regular expression, for a schema to publish beside
/// `is_hash_part`.
pub const HASH_PART_PATTERN: &str = "[0123456789abcdfghijklmnpqrsvwxyz]{32}";

/// whether `text` starts the way a store path's last component does: a hash part, then a `-` and a
/// name, or nothing.
pub fn is_hash_part(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= HASH_PART_LEN
        && bytes[..HASH_PART_LEN].iter().all(|&c| base32::is_digit(c))
        && bytes.get(HASH_PART_LEN).is_none_or(|&b| b == b'-')
}

/// `StorePath::MaxPathLen` in CppNix 2.35.3 and `StorePath::MAX_PATH_LEN` in Lix 2.95.4.
const MAX_NAME_LEN: usize = 211;

/// the directory a store lives in, such as `/nix/store`. it's part of every path's hash, so a path
/// computed for one store directory is wrong for every other.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoreDir(String);

impl StoreDir {
    pub fn new(dir: impl Into<String>) -> Result<Self, PathError> {
        let dir = dir.into();
        // Nix canonicalizes its store directory, and the directory is hashed as a string, so
        // `/nix//store` would give every path a hash no daemon computes.
        let canonical = dir.strip_prefix('/').is_some_and(|rest| {
            rest.split('/')
                .all(|c| !c.is_empty() && c != "." && c != "..")
        });
        if !canonical {
            return Err(PathError::StoreDir(dir));
        }
        Ok(Self(dir))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// the absolute form of a path in this store.
    pub fn display(&self, path: &StorePath) -> String {
        let mut out = String::with_capacity(self.0.len() + 1 + path.base_name().len());
        self.write_path(&mut out, path);
        out
    }

    /// appends the absolute form of a path, for writers that build one string from many paths.
    pub(crate) fn write_path(&self, out: &mut String, path: &StorePath) {
        out.push_str(&self.0);
        out.push('/');
        out.push_str(path.base_name());
    }

    /// reads an absolute path in this store, such as one the Nix CLI printed.
    pub fn parse(&self, text: &str) -> Result<StorePath, PathError> {
        let base = text
            .strip_prefix(self.0.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
            .ok_or_else(|| PathError::OutsideStore {
                path: text.to_owned(),
                store_dir: self.0.clone(),
            })?;
        StorePath::from_base_name(base)
    }
}

impl Default for StoreDir {
    fn default() -> Self {
        Self("/nix/store".to_owned())
    }
}

/// a valid store path name: the part after the hash and the dash.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreName(String);

impl StoreName {
    /// accepts what both CppNix 2.35.3's `checkName` (`src/libstore/path.cc`) and Lix 2.95.4's
    /// (`lix/libstore/path.cc`) accept; the two agree today, and a name either refused would be a
    /// path Shikumi computes that a daemon won't register.
    pub fn new(name: impl Into<String>) -> Result<Self, PathError> {
        let name = name.into();
        check_name(&name)?;
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StoreName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// a store path without its store directory: `<hash part>-<name>`.
///
/// ordering compares the base name as bytes, which is how CppNix and Lix order a `StorePathSet`;
/// within one store directory that's also the order of the absolute paths.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorePath(String);

impl StorePath {
    pub fn from_base_name(base: &str) -> Result<Self, PathError> {
        let bytes = base.as_bytes();
        let well_formed = bytes.len() > HASH_PART_LEN + 1
            && bytes[HASH_PART_LEN] == b'-'
            && bytes[..HASH_PART_LEN].iter().all(|&c| base32::is_digit(c));
        if !well_formed {
            return Err(PathError::HashPart(base.to_owned()));
        }
        check_name(&base[HASH_PART_LEN + 1..])?;
        Ok(Self(base.to_owned()))
    }

    pub(crate) fn from_digest(digest: &[u8; 20], name: &str) -> Self {
        Self(format!("{}-{name}", base32::encode(digest)))
    }

    pub fn base_name(&self) -> &str {
        &self.0
    }

    pub fn hash_part(&self) -> &str {
        &self.0[..HASH_PART_LEN]
    }

    pub fn name(&self) -> &str {
        &self.0[HASH_PART_LEN + 1..]
    }

    /// the name as a [`StoreName`], which it already is: a path is only built from a valid one.
    pub fn store_name(&self) -> StoreName {
        StoreName(self.name().to_owned())
    }
}

impl fmt::Display for StorePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn check_name(name: &str) -> Result<(), PathError> {
    if name.is_empty() {
        return Err(PathError::EmptyName);
    }
    if name.len() > MAX_NAME_LEN {
        return Err(PathError::NameTooLong(name.to_owned()));
    }
    // "." and "..", alone or as the first dash-separated component, would read as directory
    // references once the hash part is stripped.
    let first = name.split('-').next().unwrap_or(name);
    if first == "." || first == ".." {
        return Err(PathError::DotName(name.to_owned()));
    }
    if let Some(c) = name
        .chars()
        .find(|&c| !(c.is_ascii_alphanumeric() || "+-._?=".contains(c)))
    {
        return Err(PathError::IllegalCharacter {
            name: name.to_owned(),
            character: c,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    StoreDir(String),
    EmptyName,
    NameTooLong(String),
    DotName(String),
    IllegalCharacter { name: String, character: char },
    HashPart(String),
    OutsideStore { path: String, store_dir: String },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StoreDir(dir) => write!(
                f,
                "store directory '{dir}' must be an absolute, canonical path with no trailing slash"
            ),
            Self::EmptyName => f.write_str("a store path name can't be empty"),
            Self::NameTooLong(name) => write!(
                f,
                "store path name '{name}' is longer than {MAX_NAME_LEN} characters"
            ),
            Self::DotName(name) => write!(
                f,
                "store path name '{name}' starts with a '.' or '..' component"
            ),
            Self::IllegalCharacter { name, character } => write!(
                f,
                "store path name '{name}' contains '{character}'; only letters, digits and +-._?= are allowed"
            ),
            Self::HashPart(base) => write!(
                f,
                "'{base}' isn't a store path: it needs a {HASH_PART_LEN}-digit nix base-32 hash part, a dash and a name"
            ),
            Self::OutsideStore { path, store_dir } => {
                write!(f, "'{path}' isn't a path in the store '{store_dir}'")
            }
        }
    }
}

impl std::error::Error for PathError {}

#[cfg(test)]
mod tests {
    use super::*;

    // the pattern a schema publishes and the predicate say the same of every hash part, the
    // alphabet's letters and the four it leaves out included.
    #[test]
    fn the_hash_part_pattern_is_the_predicate() {
        let pattern = format!("^{HASH_PART_PATTERN}(-.*)?$");
        let alphabet = std::str::from_utf8(crate::base32::ALPHABET).unwrap();
        assert!(HASH_PART_PATTERN.contains(alphabet));
        for sample in [
            "0123456789abcdfghijklmnpqrsvwxyz-name".to_owned(),
            "0123456789abcdfghijklmnpqrsvwxyz".to_owned(),
            "0123456789abcdefghijklmnpqrsvwxy-name".to_owned(),
            "short-name".to_owned(),
            "0123456789abcdfghijklmnpqrsvwxyzname".to_owned(),
        ] {
            let matches = sample.len() >= HASH_PART_LEN
                && sample[..HASH_PART_LEN]
                    .chars()
                    .all(|c| alphabet.contains(c))
                && (sample.len() == HASH_PART_LEN || sample.as_bytes()[HASH_PART_LEN] == b'-');
            assert_eq!(is_hash_part(&sample), matches, "{sample} against {pattern}");
        }
    }

    #[test]
    fn names_follow_the_shared_character_rules() {
        assert!(StoreName::new("hello-2.12.tar.gz").is_ok());
        assert!(StoreName::new("a+b_c?d=e").is_ok());
        assert!(StoreName::new(".hidden").is_ok());
        assert!(StoreName::new("..x").is_ok());
        assert_eq!(StoreName::new(""), Err(PathError::EmptyName));
        assert!(matches!(
            StoreName::new("has space"),
            Err(PathError::IllegalCharacter { character: ' ', .. })
        ));
        assert!(matches!(
            StoreName::new("ünicode"),
            Err(PathError::IllegalCharacter { .. })
        ));
    }

    #[test]
    fn dot_components_are_refused() {
        for name in [".", "..", ".-x", "..-x"] {
            assert!(
                matches!(StoreName::new(name), Err(PathError::DotName(_))),
                "{name}"
            );
        }
    }

    #[test]
    fn the_name_length_limit_is_inclusive() {
        assert!(StoreName::new("a".repeat(MAX_NAME_LEN)).is_ok());
        assert!(matches!(
            StoreName::new("a".repeat(MAX_NAME_LEN + 1)),
            Err(PathError::NameTooLong(_))
        ));
    }

    #[test]
    fn absolute_paths_parse_only_inside_their_store() {
        let dir = StoreDir::default();
        let text = "/nix/store/0mdqa9w1p6cmli6976v4wi0sw9r4p5pr-hello";
        let path = dir.parse(text).unwrap();
        assert_eq!(path.hash_part(), "0mdqa9w1p6cmli6976v4wi0sw9r4p5pr");
        assert_eq!(path.name(), "hello");
        assert_eq!(dir.display(&path), text);
        assert!(matches!(
            StoreDir::new("/gnu/store").unwrap().parse(text),
            Err(PathError::OutsideStore { .. })
        ));
    }

    #[test]
    fn a_hash_part_outside_the_alphabet_is_refused() {
        // `e` isn't a nix base-32 digit.
        assert!(matches!(
            StorePath::from_base_name("emdqa9w1p6cmli6976v4wi0sw9r4p5pr-hello"),
            Err(PathError::HashPart(_))
        ));
        assert!(StorePath::from_base_name("short-hello").is_err());
    }

    #[test]
    fn store_directories_must_be_absolute_and_canonical() {
        assert!(StoreDir::new("/nix/store").is_ok());
        for dir in [
            "",
            "/",
            "nix/store",
            "/nix/store/",
            "/nix//store",
            "/nix/./store",
            "/nix/../store",
        ] {
            assert!(StoreDir::new(dir).is_err(), "{dir:?}");
        }
    }
}
