//! which Nix implementation is on the PATH, read from `nix --version`, and which derivation JSON
//! dialect its `nix derivation add` reads.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    CppNix,
    Determinate,
    Lix,
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CppNix => "CppNix",
            Self::Determinate => "Determinate Nix",
            Self::Lix => "Lix",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Implementation {
    pub family: Family,
    /// the product's own version: for Determinate Nix its release, not the CppNix it's based on.
    pub version: String,
    /// the first line of `nix --version`, as printed.
    pub banner: String,
    /// the major and minor parts of `version`.
    release: (u32, u32),
}

/// how `nix derivation add` reads a derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    /// Lix: absolute store paths, `inputSrcs` and `inputDrvs`, and a fixed output as a hex hash
    /// beside its method and algorithm (`lix/libstore/derivations.cc`, Lix 2.95.4).
    Legacy,
    /// CppNix 2.33 and later: `version: 4`, store path base names, `inputs.srcs` and
    /// `inputs.drvs`, and a fixed output as a method and an SRI hash (`src/libstore/
    /// derivations.cc`, CppNix 2.35.3; format change in `rl-2.33.md`).
    V4,
}

impl Implementation {
    /// reads `nix (Lix, like Nix) 2.95.4`, `nix (Nix) 2.35.3` and `nix (Determinate Nix 3.24.2)
    /// 3.24.2`.
    pub(crate) fn parse(version_output: &str) -> Option<Self> {
        let banner = version_output.lines().next()?.trim().to_owned();
        let rest = banner.strip_prefix("nix (")?;
        let (product, trailing) = rest.split_once(") ")?;
        let trailing = trailing.trim().to_owned();
        let (family, version) = if product.starts_with("Lix") {
            (Family::Lix, trailing)
        } else if let Some(version) = product.strip_prefix("Determinate Nix ") {
            (Family::Determinate, version.trim().to_owned())
        } else if product == "Nix" {
            (Family::CppNix, trailing)
        } else {
            return None;
        };
        let mut parts = version.split('.');
        let release = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
        Some(Self {
            family,
            version,
            banner,
            release,
        })
    }

    /// the dialect this implementation's `nix derivation add` reads, or `None` when it's a
    /// release older than any this transport writes for.
    pub(crate) fn dialect(&self) -> Option<Dialect> {
        let (major, minor) = self.release;
        match self.family {
            Family::Lix => Some(Dialect::Legacy),
            // CppNix 2.32 wrote base names in format 3 and older versions absolute paths without
            // a version; neither is something this transport writes.
            Family::CppNix => ((major, minor) >= (2, 33)).then_some(Dialect::V4),
            // Determinate Nix's banner doesn't name the CppNix it's built on. 3.24 is CppNix
            // 2.35.3 underneath (`.version` in its v3.24.2 tag); earlier releases aren't checked,
            // so they're refused rather than guessed at.
            Family::Determinate => ((major, minor) >= (3, 24)).then_some(Dialect::V4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_family_is_recognized_from_its_banner() {
        let lix = Implementation::parse("nix (Lix, like Nix) 2.95.4\nSystem type: x\n").unwrap();
        assert_eq!((lix.family, lix.version.as_str()), (Family::Lix, "2.95.4"));
        assert_eq!(lix.dialect(), Some(Dialect::Legacy));

        let cpp = Implementation::parse("nix (Nix) 2.35.3").unwrap();
        assert_eq!(
            (cpp.family, cpp.version.as_str()),
            (Family::CppNix, "2.35.3")
        );
        assert_eq!(cpp.dialect(), Some(Dialect::V4));

        let det = Implementation::parse("nix (Determinate Nix 3.24.2) 3.24.2").unwrap();
        assert_eq!(
            (det.family, det.version.as_str()),
            (Family::Determinate, "3.24.2")
        );
        assert_eq!(det.dialect(), Some(Dialect::V4));
    }

    #[test]
    fn a_cppnix_before_the_fourth_json_format_has_no_dialect() {
        let old = Implementation::parse("nix (Nix) 2.32.1").unwrap();
        assert_eq!(old.dialect(), None);
    }

    #[test]
    fn a_determinate_release_whose_base_isnt_checked_has_no_dialect() {
        let old = Implementation::parse("nix (Determinate Nix 3.8.0) 3.8.0").unwrap();
        assert_eq!(old.dialect(), None);
    }

    #[test]
    fn an_unknown_banner_or_an_unreadable_version_is_refused() {
        assert_eq!(Implementation::parse("guix (GNU Guix) 1.4.0"), None);
        assert_eq!(Implementation::parse(""), None);
        assert_eq!(Implementation::parse("nix (Nix) unstable"), None);
    }
}
