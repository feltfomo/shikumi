//! the conformance set. each case computes paths with `shikumi-derivation`, drives the daemon
//! through the store transport, and records Shikumi's value beside the daemon's.
//!
//! the daemon's values come from what it registers and reports: the path `nix derivation add`
//! returns after checking the output paths against its own computation, the output paths a build
//! reports, the NAR hash and references it records, and whether it accepts a content address from
//! an untrusted client, which it does only when it derives the same path.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use shikumi_derivation::{
    Derivation, FixedHash, Ingestion, NarNode, OutputName, Outputs, References, Sha256,
    SourceAddress, StoreDerivation, StoreDir, StoreName, StorePath,
};
use shikumi_transport::{BuildTarget, SourceNar, StoreTransport, Trust};

use crate::HarnessError;
use crate::report::Report;

const SH: &str = "/bin/sh";

struct Source {
    case: &'static str,
    address: SourceAddress,
    nar: Vec<u8>,
}

/// a synthesized derivation and the bytes each of its outputs holds once built.
struct Case {
    case: &'static str,
    drv: StoreDerivation,
    contents: BTreeMap<&'static str, Vec<u8>>,
}

pub(crate) async fn run<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    system: &str,
    report: &mut Report,
) -> Result<(), HarnessError> {
    let trust = match store.trust().await? {
        Trust::Trusted => "trusted",
        Trust::Untrusted => "untrusted",
        Trust::Unknown => "unknown",
    };
    report.push("untrusted", "client trust", "untrusted", trust);

    let mut sources = sources(dir)?;
    // dependents come first, so the transport has to order the batch itself. the NAR bytes move
    // into their readers; only the addresses are needed after the add.
    let batch: Vec<SourceNar> = sources
        .iter_mut()
        .rev()
        .map(|source| SourceNar {
            address: source.address.clone(),
            nar: Box::new(Cursor::new(std::mem::take(&mut source.nar))),
        })
        .collect();
    store.add_sources(batch).await?;
    let source_paths: Vec<StorePath> = sources
        .iter()
        .map(|source| source.address.path().clone())
        .collect();
    let valid = store.query_valid(&source_paths).await?;
    let registered: Vec<&Source> = sources
        .iter()
        .filter(|source| valid.contains(source.address.path()))
        .collect();
    for source in &sources {
        let daemon = if valid.contains(source.address.path()) {
            dir.display(source.address.path())
        } else {
            "not registered".to_owned()
        };
        report.push(
            source.case,
            "registered path",
            dir.display(source.address.path()),
            daemon,
        );
    }
    let registered_paths: Vec<StorePath> = registered
        .iter()
        .map(|source| source.address.path().clone())
        .collect();
    for (source, info) in registered
        .iter()
        .zip(store.query_path_info(&registered_paths).await?)
    {
        report.push(
            source.case,
            "nar hash",
            source.address.nar_hash().to_hex(),
            info.nar_hash.to_hex(),
        );
        report.push(
            source.case,
            "references",
            joined(source.address.references()),
            joined(&info.references),
        );
    }

    wrong_address(store, dir, "wrong-address", References::default(), report).await?;
    let plain_only = References::new(BTreeSet::from([sources[0].address.path().clone()]));
    wrong_address(store, dir, "wrong-address-referring", plain_only, report).await?;

    let cases = derivations(dir, system, &sources)?;
    // dependents first again, for the same reason.
    let ordered: Vec<&Case> = cases.iter().rev().collect();
    let drvs: Vec<&StoreDerivation> = ordered.iter().map(|case| &case.drv).collect();
    let added = store.add_derivations(&drvs).await?;
    for (Case { case, drv, .. }, added) in ordered.into_iter().zip(&added) {
        let case = *case;
        report.push(
            case,
            "drv path",
            dir.display(drv.path()),
            dir.display(added),
        );
        let stored = std::fs::read(dir.display(added)).map_or_else(
            |error| format!("unreadable: {error}"),
            |bytes| Sha256::digest(&bytes).to_hex(),
        );
        report.push(
            case,
            "drv file sha256",
            Sha256::digest(drv.aterm().as_bytes()).to_hex(),
            stored,
        );
    }

    let targets: Vec<BuildTarget> = cases
        .iter()
        .map(|case| BuildTarget {
            derivation: case.drv.path().clone(),
            outputs: case.drv.outputs().keys().cloned().collect(),
        })
        .collect();
    let built = store.build(&targets).await?;
    let mut outputs: Vec<(&'static str, &OutputName, StorePath, &[u8])> = Vec::new();
    for (case, built) in cases.iter().zip(built) {
        for (name, computed) in case.drv.outputs() {
            let daemon = built
                .get(name)
                .map_or_else(|| "not built".to_owned(), |path| dir.display(path));
            report.push(
                case.case,
                format!("output {name}"),
                dir.display(computed),
                daemon,
            );
            outputs.push((
                case.case,
                name,
                computed.clone(),
                &case.contents[name.as_str()],
            ));
        }
    }
    let output_paths: Vec<StorePath> = outputs.iter().map(|(_, _, path, _)| path.clone()).collect();
    let infos = store.query_path_info(&output_paths).await?;
    for ((case, name, path, contents), info) in outputs.iter().zip(&infos) {
        let expected = NarNode::regular(false, contents.to_vec()).to_nar()?.hash;
        report.push(
            case,
            format!("output {name} nar hash"),
            expected.to_hex(),
            info.nar_hash.to_hex(),
        );
        if *case == "self-reference" {
            self_reference(dir, path, info.nar_hash, &info.references, report)?;
        }
    }
    Ok(())
}

fn sources(dir: &StoreDir) -> Result<Vec<Source>, HarnessError> {
    let make = |case, name: &str, node: NarNode, references: References| {
        let nar = node.to_nar()?;
        let address = dir.source_address(&StoreName::new(name)?, nar.hash, references)?;
        Ok::<_, HarnessError>(Source {
            case,
            address,
            nar: nar.bytes,
        })
    };
    let plain = make(
        "source-plain",
        "plain-source",
        NarNode::regular(false, b"plain source\n".to_vec()),
        References::default(),
    )?;
    let tree = make(
        "source-tree",
        "source-tree",
        NarNode::Directory(BTreeMap::from([
            (
                "run".to_owned(),
                NarNode::regular(true, b"#!/bin/sh\necho run\n".to_vec()),
            ),
            (
                "link".to_owned(),
                NarNode::Symlink {
                    target: "run".to_owned(),
                },
            ),
            ("empty".to_owned(), NarNode::regular(false, Vec::new())),
            ("empty-run".to_owned(), NarNode::regular(true, Vec::new())),
            ("empty-dir".to_owned(), NarNode::Directory(BTreeMap::new())),
            (
                "absolute".to_owned(),
                NarNode::Symlink {
                    target: "/etc/hostname".to_owned(),
                },
            ),
            (
                "dangling".to_owned(),
                NarNode::Symlink {
                    target: "missing/../nowhere".to_owned(),
                },
            ),
            // byte order puts these as "B", "_", "a"; a case-insensitive sort wouldn't.
            ("B".to_owned(), NarNode::regular(false, b"B\n".to_vec())),
            ("_".to_owned(), NarNode::regular(false, b"_\n".to_vec())),
            ("a".to_owned(), NarNode::regular(false, b"a\n".to_vec())),
            (
                "sub".to_owned(),
                NarNode::Directory(BTreeMap::from([(
                    "data".to_owned(),
                    NarNode::regular(false, b"data\n".to_vec()),
                )])),
            ),
        ])),
        References::default(),
    )?;
    // the bytes name both paths they refer to, as a real output's would.
    let contents = format!(
        "{}\n{}\n",
        dir.display(plain.address.path()),
        dir.display(tree.address.path())
    );
    let referring = make(
        "source-references",
        "referring-source",
        NarNode::regular(false, contents.into_bytes()),
        References::new(BTreeSet::from([
            plain.address.path().clone(),
            tree.address.path().clone(),
        ])),
    )?;
    Ok(vec![plain, tree, referring])
}

/// an address computed for another store directory is a wrong path for this one, and adding it
/// must be refused. if it were registered, nothing would stand between a drifted computation and
/// a wrong path in the store.
///
/// without references, the `nix copy` client of every implementation refuses it itself, asserting
/// that the path matches its content address (`copyPaths` in CppNix 2.35.3
/// `src/libstore/store-api.cc`, and the same function in Lix 2.95.4 and 2.96.0), so the daemon never
/// sees it. with a reference the client skips that recomputation, and only the daemon's check of an
/// untrusted client's path can refuse it ("lacks a signature by a trusted key", from
/// `LocalStore::addToStore` in CppNix's `src/libstore/local-store.cc`). that second check is the one
/// the source cases rely on.
async fn wrong_address<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    case: &'static str,
    references: References,
    report: &mut Report,
) -> Result<(), HarnessError> {
    let nar = NarNode::regular(false, case.as_bytes().to_vec()).to_nar()?;
    let wrong = StoreDir::new("/shikumi/wrong-store")?.source_address(
        &StoreName::new(case)?,
        nar.hash,
        references,
    )?;
    let added = store
        .add_sources(vec![SourceNar {
            address: wrong.clone(),
            nar: Box::new(Cursor::new(nar.bytes)),
        }])
        .await;
    let valid = store
        .query_valid(std::slice::from_ref(wrong.path()))
        .await?;
    let daemon = match added {
        Err(error) if valid.is_empty() => {
            eprintln!("{case}: refused as it should be: {error}");
            "refused"
        }
        Err(_) => "registered despite an error",
        Ok(()) => "registered",
    };
    report.push(
        case,
        format!("add of {}", dir.display(wrong.path())),
        "refused",
        daemon,
    );
    Ok(())
}

/// a built output that refers to itself can't be re-added as a source. the daemon's references
/// say whether it refers to itself, and Shikumi's address computation must refuse exactly then.
fn self_reference(
    dir: &StoreDir,
    path: &StorePath,
    nar_hash: Sha256,
    references: &BTreeSet<StorePath>,
    report: &mut Report,
) -> Result<(), HarnessError> {
    let split = References::of(path, references.iter().cloned());
    let computed = match dir.source_address(&StoreName::new(path.name())?, nar_hash, split.clone())
    {
        Err(_) => "refused".to_owned(),
        Ok(address) => dir.display(address.path()),
    };
    let daemon = if split.has_self_reference() {
        "refused"
    } else {
        "no self-reference reported"
    };
    report.push(
        "self-reference",
        "source address of the output",
        computed,
        daemon,
    );
    Ok(())
}

fn derivations(
    dir: &StoreDir,
    system: &str,
    sources: &[Source],
) -> Result<Vec<Case>, HarnessError> {
    let ia = |name: &str, outputs: &[&str]| -> Result<Derivation, HarnessError> {
        let outputs = outputs
            .iter()
            .map(|output| OutputName::new(*output))
            .collect::<Result<_, _>>()?;
        Ok(Derivation::new(
            StoreName::new(name)?,
            Outputs::InputAddressed(outputs),
            system,
            SH,
        ))
    };
    let fixed = |name: &str, hash: FixedHash| -> Result<Derivation, HarnessError> {
        Ok(Derivation::new(
            StoreName::new(name)?,
            Outputs::Fixed(hash),
            system,
            SH,
        ))
    };
    let one = |bytes: &[u8]| BTreeMap::from([("out", bytes.to_vec())]);
    let mut cases = Vec::new();

    let mut drv = ia("no-inputs", &["out"])?;
    drv.arg("-c").arg("echo hello > $out");
    let no_inputs = dir.synthesize(drv)?;

    let [plain, _, referring] = sources else {
        unreachable!("sources() returns three");
    };
    let mut drv = ia("input-sources", &["out"])?;
    drv.input_source(plain.address.path().clone())
        .input_source(referring.address.path().clone())
        .env("plain", dir.display(plain.address.path()))
        .arg("-c")
        .arg("read line < \"$plain\"; echo \"$line\" > $out");
    let input_sources = dir.synthesize(drv)?;

    let mut drv = ia("multiple-outputs", &["out", "dev", "lib"])?;
    drv.arg("-c")
        .arg("echo out > $out; echo dev > $dev; echo lib > $lib");
    let multiple = dir.synthesize(drv)?;

    let mut drv = fixed(
        "fixed-flat",
        FixedHash {
            ingestion: Ingestion::Flat,
            sha256: Sha256::digest(b"fixed\n"),
        },
    )?;
    drv.arg("-c").arg("echo fixed > $out");
    let fixed_flat = dir.synthesize(drv)?;

    let mut drv = fixed(
        "fixed-recursive",
        FixedHash {
            ingestion: Ingestion::Recursive,
            sha256: NarNode::regular(false, b"fixed\n".to_vec()).to_nar()?.hash,
        },
    )?;
    drv.arg("-c").arg("echo fixed > $out");
    let fixed_recursive = dir.synthesize(drv)?;

    let mut drv = ia("input-derivations", &["out"])?;
    drv.input_derivation(&no_inputs, ["out"])?
        .input_derivation(&multiple, ["dev", "lib"])?
        .input_derivation(&fixed_flat, ["out"])?
        .input_derivation(&fixed_recursive, ["out"])?
        .env("a", dir.display(&no_inputs.outputs()["out"]))
        .env("b", dir.display(&multiple.outputs()["dev"]))
        .arg("-c")
        .arg("read a < \"$a\"; read b < \"$b\"; echo \"$a$b\" > $out");
    let input_derivations = dir.synthesize(drv)?;

    // two fetchers of one content under one name, built differently: one input to a dependent.
    let merged_hash = FixedHash {
        ingestion: Ingestion::Recursive,
        sha256: NarNode::regular(false, b"merged\n".to_vec()).to_nar()?.hash,
    };
    let mut drv = fixed("merged-fetch", merged_hash)?;
    drv.arg("-c").arg("echo merged > $out");
    let merged_a = dir.synthesize(drv)?;
    let mut drv = fixed("merged-fetch", merged_hash)?;
    drv.arg("-c").arg("printf 'merged\\n' > $out");
    let merged_b = dir.synthesize(drv)?;
    let mut drv = ia("fixed-merged", &["out"])?;
    drv.input_derivation(&merged_a, ["out"])?
        .input_derivation(&merged_b, ["out"])?
        .arg("-c")
        .arg("echo merged-user > $out");
    let fixed_merged = dir.synthesize(drv)?;

    // every character `printString` escapes, and the ones it leaves alone.
    let mut drv = ia("unusual-strings", &["out"])?;
    drv.env("quote", "a \"b\" c")
        .env("backslash", "c:\\d\\")
        .env("newline", "one\ntwo\n")
        .env("crlf", "a\r\nb")
        .env("tab", "a\tb")
        .env("empty", "")
        .env("unicode", "ünïcödé 日本 🦀")
        .env("shell", "$out ${x} `y` 'z'")
        .env("key with spaces", "v")
        .env("", "empty key")
        .env("Z", "upper")
        .env("_x", "underscore")
        .env("é", "non-ascii key")
        .env("~", "tilde")
        .arg("-c")
        .arg("echo strings > $out")
        .arg("")
        .arg("arg \"quoted\"")
        .arg("back\\slash")
        .arg("multi\nline\ttab");
    let unusual = dir.synthesize(drv)?;

    let mut drv = ia("self-reference", &["out"])?;
    drv.arg("-c").arg("echo $out > $out");
    let self_ref = dir.synthesize(drv)?;
    let self_contents = format!("{}\n", dir.display(self_ref.output("out").expect("out")));

    // a fixed output's inputs reach its `.drv` path but not its output path or its hash modulo.
    let mut drv = fixed(
        "fixed-with-inputs",
        FixedHash {
            ingestion: Ingestion::Flat,
            sha256: Sha256::digest(b"fixed inputs\n"),
        },
    )?;
    drv.input_source(plain.address.path().clone())
        .input_derivation(&no_inputs, ["out"])?
        .arg("-c")
        .arg("echo fixed inputs > $out");
    let fixed_with_inputs = dir.synthesize(drv)?;

    // two input-addressed derivations that differ only in which of two equivalent fetchers they
    // use share a hash modulo, so a dependent naming one's `out` and the other's `dev` sees one
    // input with both outputs.
    let member = |fetch: &StoreDerivation| -> Result<StoreDerivation, HarnessError> {
        let mut drv = ia("merge-member", &["out", "dev"])?;
        drv.input_derivation(fetch, ["out"])?
            .arg("-c")
            .arg("echo m > $out; echo d > $dev");
        Ok(dir.synthesize(drv)?)
    };
    let (member_a, member_b) = (member(&merged_a)?, member(&merged_b)?);
    let mut drv = ia("regular-merged", &["out"])?;
    drv.input_derivation(&member_a, ["out"])?
        .input_derivation(&member_b, ["dev"])?
        .arg("-c")
        .arg("echo regular-merged > $out");
    let regular_merged = dir.synthesize(drv)?;

    // the daemon reads that `.drv` back to hash this one, so its escapes are parsed too.
    let mut drv = ia("strings-dependent", &["out"])?;
    drv.input_derivation(&unusual, ["out"])?
        .arg("-c")
        .arg("echo strings-dependent > $out");
    let strings_dependent = dir.synthesize(drv)?;

    // two levels of input-addressed inputs, beside a source: every input here has inputs of its
    // own, so writing them as paths instead of hashes modulo would change this path.
    let mut drv = ia("chain", &["out"])?;
    drv.input_source(referring.address.path().clone())
        .input_derivation(&input_derivations, ["out"])?
        .input_derivation(&fixed_merged, ["out"])?
        .input_derivation(&regular_merged, ["out"])?
        .env("x", dir.display(&input_derivations.outputs()["out"]))
        .arg("-c")
        .arg("read x < \"$x\"; echo \"chain $x\" > $out");
    let chain = dir.synthesize(drv)?;

    cases.push(Case {
        case: "no-inputs",
        drv: no_inputs,
        contents: one(b"hello\n"),
    });
    cases.push(Case {
        case: "input-sources",
        drv: input_sources,
        contents: one(b"plain source\n"),
    });
    cases.push(Case {
        case: "multiple-outputs",
        drv: multiple,
        contents: BTreeMap::from([
            ("out", b"out\n".to_vec()),
            ("dev", b"dev\n".to_vec()),
            ("lib", b"lib\n".to_vec()),
        ]),
    });
    cases.push(Case {
        case: "fixed-flat",
        drv: fixed_flat,
        contents: one(b"fixed\n"),
    });
    cases.push(Case {
        case: "fixed-recursive",
        drv: fixed_recursive,
        contents: one(b"fixed\n"),
    });
    cases.push(Case {
        case: "input-derivations",
        drv: input_derivations,
        contents: one(b"hellodev\n"),
    });
    cases.push(Case {
        case: "fixed-merged",
        drv: merged_a,
        contents: one(b"merged\n"),
    });
    cases.push(Case {
        case: "fixed-merged",
        drv: merged_b,
        contents: one(b"merged\n"),
    });
    cases.push(Case {
        case: "fixed-merged",
        drv: fixed_merged,
        contents: one(b"merged-user\n"),
    });
    cases.push(Case {
        case: "unusual-strings",
        drv: unusual,
        contents: one(b"strings\n"),
    });
    cases.push(Case {
        case: "self-reference",
        drv: self_ref,
        contents: one(self_contents.as_bytes()),
    });
    cases.push(Case {
        case: "fixed-with-inputs",
        drv: fixed_with_inputs,
        contents: one(b"fixed inputs\n"),
    });
    let member_contents = || BTreeMap::from([("out", b"m\n".to_vec()), ("dev", b"d\n".to_vec())]);
    cases.push(Case {
        case: "regular-merged",
        drv: member_a,
        contents: member_contents(),
    });
    cases.push(Case {
        case: "regular-merged",
        drv: member_b,
        contents: member_contents(),
    });
    cases.push(Case {
        case: "regular-merged",
        drv: regular_merged,
        contents: one(b"regular-merged\n"),
    });
    cases.push(Case {
        case: "strings-dependent",
        drv: strings_dependent,
        contents: one(b"strings-dependent\n"),
    });
    cases.push(Case {
        case: "chain",
        drv: chain,
        contents: one(b"chain hellodev\n"),
    });
    Ok(cases)
}

fn joined(paths: &BTreeSet<StorePath>) -> String {
    let names: Vec<&str> = paths.iter().map(StorePath::base_name).collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(" ")
    }
}
