//! the repository's lock, `shikumi.lock/`: every input Shikumi fetches, pinned by hash, as JSON
//! lines. a repository has one, at its root beside the manifest.
//!
//! the lock is a directory of shards. `lock.json` names the contract and its version, and every
//! other file holds the entries of one package of one ecosystem, `<ecosystem>/<package>.jsonl`,
//! one entry per line, sorted. two changes to different packages touch different files, so they
//! merge without a conflict, and each line is one artifact a person can find with grep. every line
//! has to be written exactly as Shikumi writes it, its `\n` included, so a lock has one spelling
//! and a review sees only real changes.
//!
//! the ecosystems are a closed set, and each entry is an object tagged with its ecosystem's name.
//! an ecosystem added later is a new tag with a shape of its own, which leaves every existing line
//! as it is.
//!
//! reading costs one pass over the tracked lock files, linear in the entries.

pub mod cli;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use shikumi_contract::{closed_set, contract_name, contract_version};
use shikumi_derivation::{FixedHash, Ingestion, Sha256, StoreDir, StorePath};

pub use shikumi_contract::render_json;

/// where the lock lives, relative to the repository's root.
pub const DIRECTORY: &str = "shikumi.lock";

/// the file inside the lock that names its contract and version.
pub const HEADER: &str = "lock.json";

/// the extension every shard has.
const SHARD_EXTENSION: &str = "jsonl";

/// the store name an unpacked toolchain source has, which is also the name a flake input's source
/// and a `fetchTarball` without one get, so the same tree pinned any of these ways is one path.
pub const SOURCE_NAME: &str = "source";

closed_set! {
    /// a kind of artifact the lock pins: one shard directory and one entry shape each.
    Ecosystem {
        Toolchain = "toolchain": "a source tree Shikumi's own Nix runs on: an unpacked tarball, by the NAR hash of its tree",
    }
}

/// how many entries one package's shard holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerPackage {
    /// exactly one: pinning the package again replaces it.
    One,
}

impl Ecosystem {
    pub const fn per_package(self) -> PerPackage {
        match self {
            Self::Toolchain => PerPackage::One,
        }
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|ecosystem| ecosystem.as_str() == name)
    }
}

closed_set! {
    /// a source tree Shikumi's own Nix needs.
    ToolchainSource {
        Nixpkgs = "nixpkgs": "the nixpkgs the declaration layer runs on and the catalog reads",
        NixUnit = "nix-unit": "the nix-unit a repository's evaluation tests run with",
    }
}

impl ToolchainSource {
    /// the source named `name`, or why there's none, as the contract and the command line both
    /// say it.
    pub fn named(name: &str) -> Result<Self, String> {
        Self::ALL
            .iter()
            .copied()
            .find(|source| source.as_str() == name)
            .ok_or_else(|| {
                format!(
                    "{name:?} isn't a toolchain source; the lock pins {}",
                    quoted(Self::ALL.iter().map(|source| source.as_str()))
                )
            })
    }
}

/// where a pinned artifact is fetched from: an `https`, `http` or `file` URL.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Url(String);

const URL_SCHEMES: [&str; 3] = ["https://", "http://", "file://"];

impl Url {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let scheme = URL_SCHEMES
            .into_iter()
            .find(|scheme| value.starts_with(scheme));
        match scheme {
            Some(scheme)
                if value.len() > scheme.len()
                    && !value.chars().any(|c| c.is_whitespace() || c.is_control()) =>
            {
                Ok(Self(value))
            }
            _ => Err(format!(
                "{value:?} isn't a URL Shikumi fetches: it starts with {} and has no whitespace",
                quoted(URL_SCHEMES.into_iter())
            )),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Url {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Url> for String {
    fn from(value: Url) -> Self {
        value.0
    }
}

impl JsonSchema for Url {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Url".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({ "type": "string", "pattern": "^(https?|file)://\\S+$" })
    }
}

/// the sha256 of a tree's NAR serialization, written in SRI form, `sha256-<base64>`, as a flake's
/// lock writes `narHash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NarHash(Sha256);

impl NarHash {
    pub fn new(sha256: Sha256) -> Self {
        Self(sha256)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        Sha256::parse_sri(text)
            .map(Self)
            .map_err(|_| format!("{text:?} isn't a sha256 written as `sha256-<base64>`"))
    }

    pub fn sha256(&self) -> &Sha256 {
        &self.0
    }
}

impl fmt::Display for NarHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_sri())
    }
}

impl TryFrom<String> for NarHash {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<NarHash> for String {
    fn from(value: NarHash) -> Self {
        value.0.to_sri()
    }
}

impl JsonSchema for NarHash {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "NarHash".into()
    }

    // the last digit before the padding carries four bits of the hash and two that are zero.
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({ "type": "string", "pattern": "^sha256-[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]=$" })
    }
}

/// a toolchain source, fetched from `url` and unpacked, whose tree has the NAR hash `narHash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolchainPin {
    pub name: ToolchainSource,
    pub url: Url,
    pub nar_hash: NarHash,
}

impl ToolchainPin {
    /// the store path the unpacked source has: a recursive fixed output named `source`, whichever
    /// fetcher put it there.
    pub fn store_path(&self, store: &StoreDir) -> StorePath {
        store.fixed_output_path(
            SOURCE_NAME,
            &FixedHash {
                ingestion: Ingestion::Recursive,
                sha256: *self.nar_hash.sha256(),
            },
        )
    }
}

/// one line of the lock: an artifact of one ecosystem, tagged with the ecosystem's name. each
/// variant's tag is `Ecosystem`'s name for the variant `ecosystem` maps it to, which a test holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "ecosystem", deny_unknown_fields)]
#[schemars(title = "shikumi.lock entry")]
pub enum Entry {
    #[serde(rename = "toolchain")]
    Toolchain(ToolchainPin),
}

impl Entry {
    pub fn ecosystem(&self) -> Ecosystem {
        match self {
            Self::Toolchain(_) => Ecosystem::Toolchain,
        }
    }

    /// the package the entry pins, which names its shard.
    pub fn package(&self) -> &str {
        match self {
            Self::Toolchain(pin) => pin.name.as_str(),
        }
    }

    /// the shard the entry belongs in, relative to the repository's root.
    pub fn shard(&self) -> String {
        shard(self.ecosystem(), self.package())
    }

    /// the line as the lock holds it, without its `\n`.
    pub fn line(&self) -> String {
        serde_json::to_string(self).expect("a lock entry always serializes")
    }

    /// reads one line, which has to be written exactly as `line` writes it.
    pub fn parse(line: &str) -> Result<Self, String> {
        let entry: Self = serde_json::from_str(line).map_err(|error| error.to_string())?;
        let written = entry.line();
        if written == line {
            Ok(entry)
        } else {
            Err(format!(
                "the entry isn't written as Shikumi writes it, which is:\n{written}"
            ))
        }
    }
}

/// the shard for a package of an ecosystem, relative to the repository's root. the one place the
/// lock's layout is written.
pub fn shard(ecosystem: Ecosystem, package: &str) -> String {
    format!("{DIRECTORY}/{ecosystem}/{package}.{SHARD_EXTENSION}")
}

/// the ecosystem and package a shard's path names, or nothing for a path that isn't one.
fn shard_of(path: &str) -> Option<(Ecosystem, &str)> {
    let inside = path.strip_prefix(DIRECTORY)?.strip_prefix('/')?;
    let (ecosystem, file) = inside.split_once('/')?;
    let package = file.strip_suffix(SHARD_EXTENSION)?.strip_suffix('.')?;
    let ecosystem = Ecosystem::named(ecosystem)?;
    (!package.is_empty() && !package.contains('/')).then_some((ecosystem, package))
}

/// the header's path, relative to the repository's root.
pub fn header() -> String {
    format!("{DIRECTORY}/{HEADER}")
}

contract_name!(LockName, "shikumi.lock");

contract_version!(
    /// the lock's version.
    LockVersion,
    1,
    0
);

/// `lock.json`: the contract's name and version.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "shikumi.lock")]
struct Header {
    schema: LockName,
    version: LockVersion,
}

impl Header {
    /// the header as `lock.json` holds it.
    fn text() -> String {
        render_json(&Header {
            schema: LockName,
            version: LockVersion,
        })
    }

    fn parse(text: &str) -> Result<(), String> {
        serde_json::from_str::<Header>(text).map_err(|error| error.to_string())?;
        if text == Self::text() {
            Ok(())
        } else {
            Err(format!(
                "the header isn't written as Shikumi writes it, which is:\n{}",
                Self::text()
            ))
        }
    }
}

/// the published schema of `lock.json`.
pub fn header_schema() -> Schema {
    schemars::schema_for!(Header)
}

/// the published schema of one line of a shard.
pub fn entry_schema() -> Schema {
    schemars::schema_for!(Entry)
}

/// a shard's lines, each without its `\n`. a shard ends every line with `\n` alone, so a `\r` stays
/// in the line, where the check against how Shikumi writes it refuses it.
fn lines(text: &str) -> Result<Vec<&str>, String> {
    match text.strip_suffix('\n') {
        Some(body) if !body.is_empty() => Ok(body.split('\n').collect()),
        _ => Err("a shard holds at least one entry, each on a line of its own".to_owned()),
    }
}

/// the lock as its tracked files hold it, every entry in shard order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    entries: Vec<Entry>,
}

impl Lock {
    /// reads the lock from `files`, the tracked files under `DIRECTORY`, relative to `root`. an
    /// untracked file isn't part of the repository, so it isn't part of the lock either.
    pub fn read<'a>(
        root: &Path,
        files: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, LockError> {
        let mut headed = false;
        let mut shards = Vec::new();
        for file in files {
            if file == header() {
                headed = true;
                Header::parse(&read(root, file)?).map_err(|message| LockError::Header {
                    file: file.to_owned(),
                    message,
                })?;
            } else if let Some((ecosystem, package)) = shard_of(file) {
                shards.push((file, ecosystem, package));
            } else {
                return Err(LockError::Stray(file.to_owned()));
            }
        }
        if !headed {
            return Err(LockError::NoLock(root.to_owned()));
        }
        shards.sort_unstable();
        let mut entries = Vec::new();
        for (file, ecosystem, package) in shards {
            let text = read(root, file)?;
            let at = |line: usize, message: String| LockError::Line {
                file: file.to_owned(),
                line,
                message,
            };
            let lines = lines(&text).map_err(|message| at(1, message))?;
            for (index, line) in lines.iter().enumerate() {
                let at = |message: String| at(index + 1, message);
                let entry = Entry::parse(line).map_err(at)?;
                if entry.ecosystem() != ecosystem || entry.package() != package {
                    return Err(at(format!("the entry belongs in {}", entry.shard())));
                }
                if index > 0 && lines[index - 1] >= *line {
                    return Err(at(
                        "the shard's lines are sorted, each once, and this one isn't".to_owned(),
                    ));
                }
                if index > 0 && ecosystem.per_package() == PerPackage::One {
                    return Err(at(format!("a {ecosystem} package is pinned once")));
                }
                entries.push(entry);
            }
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn toolchain(&self, source: ToolchainSource) -> Option<&ToolchainPin> {
        self.entries.iter().find_map(|entry| match entry {
            Entry::Toolchain(pin) if pin.name == source => Some(pin),
            Entry::Toolchain(_) => None,
        })
    }

    /// a toolchain source the caller can't run without.
    pub fn required(&self, source: ToolchainSource) -> Result<&ToolchainPin, LockError> {
        self.toolchain(source).ok_or(LockError::Unpinned(source))
    }
}

fn read(root: &Path, file: &str) -> Result<String, LockError> {
    std::fs::read_to_string(root.join(file)).map_err(|error| LockError::Read {
        file: file.to_owned(),
        error,
    })
}

/// writes `entry` into its shard at `root`, and the header if the lock has none yet. each file is
/// written whole and renamed into place, so a reader never sees half of one. returns the files
/// written, relative to `root`.
pub fn pin(root: &Path, entry: &Entry) -> Result<Vec<String>, LockError> {
    let mut written = Vec::new();
    let header = header();
    if !root.join(&header).is_file() {
        replace(root, &header, &Header::text())?;
        written.push(header);
    }
    let shard = entry.shard();
    let lines = match entry.ecosystem().per_package() {
        // the package's one entry is the one being pinned, whatever the shard held.
        PerPackage::One => vec![entry.line()],
    };
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    replace(root, &shard, &text)?;
    written.push(shard);
    Ok(written)
}

fn replace(root: &Path, file: &str, text: &str) -> Result<(), LockError> {
    let path = root.join(file);
    let io = |error| LockError::Write {
        file: file.to_owned(),
        error,
    };
    let directory = path.parent().expect("a lock file is inside the lock");
    std::fs::create_dir_all(directory).map_err(io)?;
    let temporary = directory.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("shard"),
        std::process::id()
    ));
    std::fs::write(&temporary, text).map_err(io)?;
    std::fs::rename(&temporary, &path).map_err(io)
}

#[derive(Debug)]
pub enum LockError {
    NoLock(PathBuf),
    Stray(String),
    Read {
        file: String,
        error: io::Error,
    },
    Write {
        file: String,
        error: io::Error,
    },
    Header {
        file: String,
        message: String,
    },
    Line {
        file: String,
        line: usize,
        message: String,
    },
    Unpinned(ToolchainSource),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLock(root) => write!(
                f,
                "{} has no lock: Shikumi reads every pin from {DIRECTORY}/{HEADER} and the shards \
                 beside it, as Git tracks them. pin nixpkgs with `shikumi lock pin nixpkgs <url>` \
                 and add {DIRECTORY}/ to Git",
                root.display()
            ),
            Self::Stray(file) => write!(
                f,
                "{file} is in the lock but isn't {} or a shard, which is \
                 {DIRECTORY}/<ecosystem>/<package>.{SHARD_EXTENSION} for an ecosystem of {}",
                header(),
                quoted(Ecosystem::ALL.iter().map(|ecosystem| ecosystem.as_str()))
            ),
            Self::Read { file, error } => write!(f, "reading {file}: {error}"),
            Self::Write { file, error } => write!(f, "writing {file}: {error}"),
            Self::Header { file, message } => write!(f, "{file}: {message}"),
            Self::Line {
                file,
                line,
                message,
            } => write!(f, "{file}:{line}: {message}"),
            Self::Unpinned(source) => write!(
                f,
                "the lock pins no {source}; pin it with `shikumi lock pin {source} <url>`"
            ),
        }
    }
}

impl std::error::Error for LockError {}

/// names as a sentence lists them: `` `a`, `b` or `c` ``.
fn quoted<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let named: Vec<String> = names.map(|name| format!("`{name}`")).collect();
    match named.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => named.concat(),
    }
}

#[cfg(test)]
mod tests;
