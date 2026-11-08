//! how a digest, a type and a name become a store path. every path Shikumi computes, for outputs,
//! `.drv` files and content-addressed sources alike, goes through `make_store_path` here.
//!
//! the rules follow `makeStorePath`, `makeType`, `makeFixedOutputPath` and `makeOutputPath` in
//! CppNix 2.35.3 `src/libstore/store-dir-config.cc`, and `compressHash` in `src/libutil/hash.cc`.
//! Lix 2.95.4 and 2.96.0 compute the same strings in `lix/libstore/store-api.cc`.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;

use sha2::Digest;

use crate::{Sha256, StoreDir, StoreName, StorePath};

/// how a fixed output's or a source's bytes are hashed: the file itself, or its NAR serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ingestion {
    Flat,
    Recursive,
}

/// a fixed-output derivation's declared output hash. only sha256 is supported, the algorithm every
/// implementation and every lock entry Shikumi writes uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FixedHash {
    pub ingestion: Ingestion,
    pub sha256: Sha256,
}

impl FixedHash {
    /// the method and algorithm as a `.drv` file and the hash modulo write them: `r:sha256` or
    /// `sha256`.
    pub fn method_algo(&self) -> &'static str {
        match self.ingestion {
            Ingestion::Flat => "sha256",
            Ingestion::Recursive => "r:sha256",
        }
    }
}

/// a path's references, split the way the path's hash sees them: other paths, and whether it
/// refers to itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    others: BTreeSet<StorePath>,
    self_reference: bool,
}

impl References {
    pub fn new(others: BTreeSet<StorePath>) -> Self {
        Self {
            others,
            self_reference: false,
        }
    }

    /// splits the references a daemon reports for `owner`, where a reference to `owner` itself is a
    /// self-reference rather than an ordinary one.
    pub fn of(owner: &StorePath, references: impl IntoIterator<Item = StorePath>) -> Self {
        let mut out = Self::default();
        for reference in references {
            if &reference == owner {
                out.self_reference = true;
            } else {
                out.others.insert(reference);
            }
        }
        out
    }

    pub fn others(&self) -> &BTreeSet<StorePath> {
        &self.others
    }

    pub fn has_self_reference(&self) -> bool {
        self.self_reference
    }
}

/// a content-addressed source path: NAR bytes with their recursive sha256, a name and references,
/// registered the way the daemon registers a `source`-type path. it can only be built by
/// [`StoreDir::source_address`], so its path always matches its hash and references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceAddress {
    path: StorePath,
    nar_hash: Sha256,
    references: BTreeSet<StorePath>,
}

impl SourceAddress {
    pub fn path(&self) -> &StorePath {
        &self.path
    }

    /// what the daemon is told this path's content is, which it recomputes the path from.
    pub fn content_address(&self) -> ContentAddress {
        ContentAddress::Recursive(self.nar_hash)
    }

    pub fn nar_hash(&self) -> &Sha256 {
        &self.nar_hash
    }

    pub fn references(&self) -> &BTreeSet<StorePath> {
        &self.references
    }
}

/// how a content-addressed path's bytes are described to a daemon. the daemon recomputes the path
/// from this and the references, and an untrusted client's path that doesn't reproduce is refused,
/// so this and the path computed here have to agree; both come from this module. it renders as
/// `renderContentAddress` does in CppNix 2.35.3 `src/libstore/content-address.cc`, which Lix 2.95.4
/// and 2.96.0 parse alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentAddress {
    /// a text file, such as a `.drv`, hashed as it is: `text:sha256:<nix32>`.
    Text(Sha256),
    /// NAR bytes hashed recursively, as a source is: `fixed:r:sha256:<nix32>`.
    Recursive(Sha256),
}

impl fmt::Display for ContentAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(hash) => write!(f, "text:sha256:{}", hash.to_base32()),
            Self::Recursive(hash) => write!(f, "fixed:r:sha256:{}", hash.to_base32()),
        }
    }
}

/// a content-addressed source can't name its own path: the path is a hash of the bytes, so bytes
/// containing it can't exist without a rewrite Shikumi doesn't do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfReference {
    pub name: String,
}

impl fmt::Display for SelfReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' refers to itself, and a content-addressed source path can't",
            self.name
        )
    }
}

impl std::error::Error for SelfReference {}

impl StoreDir {
    /// the path the daemon registers for NAR bytes with this hash, under this name, with these
    /// references, as a `fixed:r:sha256` content address.
    pub fn source_address(
        &self,
        name: &StoreName,
        nar_hash: Sha256,
        references: References,
    ) -> Result<SourceAddress, SelfReference> {
        // CppNix would hash a `:self` suffix into the type here, but the bytes would then have to
        // be hashed modulo their own path, and Lix's support for that isn't something the
        // conformance set proves. re-addressing never needs it, so it's refused.
        if references.self_reference {
            return Err(SelfReference {
                name: name.as_str().to_owned(),
            });
        }
        Ok(SourceAddress {
            path: self.make_store_path("source", &references.others, &nar_hash, name.as_str()),
            nar_hash,
            references: references.others,
        })
    }

    /// the path of a text file added with these references, which is how a `.drv` file is
    /// addressed.
    pub fn text_path(
        &self,
        name: &str,
        contents: &[u8],
        references: &BTreeSet<StorePath>,
    ) -> StorePath {
        self.make_store_path("text", references, &Sha256::digest(contents), name)
    }

    /// a `.drv` file's path: text-addressed under `<name>.drv`, referring to its input sources and
    /// input derivations together, in one sorted set.
    pub(crate) fn drv_path<'a>(
        &self,
        name: &str,
        aterm_hash: &Sha256,
        sources: &'a BTreeSet<StorePath>,
        derivations: impl Iterator<Item = &'a StorePath>,
    ) -> StorePath {
        let mut references: Vec<&StorePath> = sources.iter().chain(derivations).collect();
        references.sort_unstable();
        references.dedup();
        self.make_store_path("text", references, aterm_hash, &format!("{name}.drv"))
    }

    pub fn fixed_output_path(&self, name: &str, hash: &FixedHash) -> StorePath {
        match hash.ingestion {
            // recursive sha256 is the one fixed output that shares the source path scheme, which
            // is why `nix-store --add` and a recursive fetch of the same tree agree.
            Ingestion::Recursive => self.make_store_path("source", [], &hash.sha256, name),
            Ingestion::Flat => {
                self.make_store_path("output:out", [], &fixed_fingerprint(hash, ""), name)
            }
        }
    }

    /// what a fixed-output derivation contributes to its dependents' hashes in place of itself.
    pub(crate) fn fixed_modulo(&self, hash: &FixedHash, output: &StorePath) -> Sha256 {
        fixed_fingerprint(hash, &self.display(output))
    }

    /// an input-addressed output's path, from its derivation's hash modulo with outputs masked.
    pub(crate) fn output_path(&self, output: &str, drv_hash: &Sha256, drv_name: &str) -> StorePath {
        let kind = format!("output:{output}");
        let name = output_path_name(drv_name, output);
        self.make_store_path(&kind, [], drv_hash, &name)
    }

    /// `name` must already be a valid store name: every caller holds a `StoreName` or one built
    /// from one plus a suffix the derivation checks. the fingerprint is
    /// `<kind>[:<reference>...]:sha256:<hex>:<store dir>:<name>`, fed to the hash in pieces rather
    /// than formatted, since a `.drv` can carry thousands of references.
    fn make_store_path<'a>(
        &self,
        kind: &str,
        references: impl IntoIterator<Item = &'a StorePath>,
        hash: &Sha256,
        name: &str,
    ) -> StorePath {
        let mut hasher = sha2::Sha256::new();
        hasher.update(kind);
        for reference in references {
            hasher.update(":");
            hasher.update(self.as_str());
            hasher.update("/");
            hasher.update(reference.base_name());
        }
        let mut hex = [0u8; 64];
        hasher.update(":sha256:");
        hasher.update(hash.write_hex(&mut hex));
        hasher.update(":");
        hasher.update(self.as_str());
        hasher.update(":");
        hasher.update(name);
        let digest = Sha256::from_bytes(hasher.finalize().into());
        StorePath::from_digest(&compress(&digest), name)
    }
}

/// `fixed:out:<method and algorithm>:<hex>:<path>`, the one fixed-output fingerprint. with the
/// path empty it's the payload of a flat output's path (`makeFixedOutputPath` in CppNix 2.35.3
/// `src/libstore/store-dir-config.cc` writes `fixed:out:sha256:<hex>:`); with the output's path
/// it's the hash modulo a dependent sees (`hashDerivationModulo` in `src/libstore/derivations.cc`).
fn fixed_fingerprint(hash: &FixedHash, path: &str) -> Sha256 {
    let mut hex = [0u8; 64];
    let mut hasher = sha2::Sha256::new();
    hasher.update("fixed:out:");
    hasher.update(hash.method_algo());
    hasher.update(":");
    hasher.update(hash.sha256.write_hex(&mut hex));
    hasher.update(":");
    hasher.update(path);
    Sha256::from_bytes(hasher.finalize().into())
}

/// the path name of a derivation's output: the derivation's name for `out`, `<name>-<output>`
/// otherwise.
pub(crate) fn output_path_name<'a>(drv_name: &'a str, output: &str) -> Cow<'a, str> {
    if output == "out" {
        Cow::Borrowed(drv_name)
    } else {
        Cow::Owned(format!("{drv_name}-{output}"))
    }
}

/// folds a 32-byte digest into the 20 bytes a hash part encodes, by XOR.
fn compress(digest: &Sha256) -> [u8; 20] {
    let mut out = [0u8; 20];
    for (i, &b) in digest.as_bytes().iter().enumerate() {
        out[i % 20] ^= b;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> StoreName {
        StoreName::new(text).unwrap()
    }

    // `builtins.toFile "hello.txt" "hello\n"` on Lix 2.95.4. a change here moves every `.drv`
    // path, since a `.drv` is addressed as a text file.
    #[test]
    fn a_text_file_without_references_matches_nix() {
        let path = StoreDir::default().text_path("hello.txt", b"hello\n", &BTreeSet::new());
        assert_eq!(
            path.base_name(),
            "qa1w9gdfrba6jl2r57mb3c43863gqywp-hello.txt"
        );
    }

    // `nix-store --add` of a file holding "hello\n", named `hello.txt`, on Lix 2.95.4: a source
    // path with no references. a change here moves every re-addressed output and every source.
    #[test]
    fn a_source_without_references_matches_nix_store_add() {
        let nar = crate::NarNode::regular(false, b"hello\n".to_vec())
            .to_nar()
            .unwrap();
        let address = StoreDir::default()
            .source_address(&name("hello.txt"), nar.hash, References::default())
            .unwrap();
        assert_eq!(
            address.path().base_name(),
            "i9pmrzmpshapij2kin22pff6fc2adavx-hello.txt"
        );
    }

    // the content address of the last path info in CppNix 2.35.3's
    // `src/libstore-tests/data/worker-protocol/valid-path-info-1.16.bin`, whose hash is
    // `sha256-EMIJ+giQ/gLIWoxmPKjno3zHZrxbGymgzGGyZvZBIdM=`. a change here makes the daemon refuse
    // every source and `.drv` Shikumi adds.
    #[test]
    fn a_content_address_renders_as_nix_writes_it() {
        let hash =
            Sha256::parse_hex("10c209fa0890fe02c85a8c663ca8e7a37cc766bc5b1b29a0cc61b266f64121d3")
                .unwrap();
        assert_eq!(
            ContentAddress::Recursive(hash).to_string(),
            "fixed:r:sha256:1lr187v6dck1rjh2j6svpikcfz53wyl3qrlcbb405zlh13x0khhh"
        );
        assert_eq!(
            ContentAddress::Text(hash).to_string(),
            "text:sha256:1lr187v6dck1rjh2j6svpikcfz53wyl3qrlcbb405zlh13x0khhh"
        );
    }

    #[test]
    fn a_self_reference_is_refused() {
        let dir = StoreDir::default();
        let owner = dir
            .source_address(&name("x"), Sha256::digest(b"x"), References::default())
            .unwrap()
            .path()
            .clone();
        let references = References::of(&owner, [owner.clone()]);
        assert!(references.has_self_reference());
        assert_eq!(
            dir.source_address(&name("x"), Sha256::digest(b"x"), references),
            Err(SelfReference {
                name: "x".to_owned()
            })
        );
    }

    #[test]
    fn a_recursive_sha256_fixed_output_is_a_source_path() {
        let dir = StoreDir::default();
        let hash = Sha256::digest(b"nar");
        let fixed = dir.fixed_output_path(
            "x",
            &FixedHash {
                ingestion: Ingestion::Recursive,
                sha256: hash,
            },
        );
        let source = dir
            .source_address(&name("x"), hash, References::default())
            .unwrap();
        assert_eq!(&fixed, source.path());
    }

    #[test]
    fn the_store_directory_is_part_of_the_hash() {
        let other = StoreDir::new("/gnu/store").unwrap();
        let refs = BTreeSet::new();
        assert_ne!(
            StoreDir::default().text_path("a", b"a", &refs),
            other.text_path("a", b"a", &refs)
        );
    }
}
