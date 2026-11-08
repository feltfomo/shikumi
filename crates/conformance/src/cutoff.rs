//! early cutoff through re-addressing, checked against the daemon end to end: a unit A, a unit B
//! that names A's re-addressed output, and rebuilds of A that keep or change its bytes.
//!
//! every value the cases compare comes from the daemon or from a measurement: whether a path is
//! valid before anything builds it, the paths a build reports, and the NAR bytes a re-add read
//! and sent. nothing is inferred from what re-addressing is meant to do, so a broken step fails a
//! check rather than passing one it never reached.

use std::collections::{BTreeMap, BTreeSet};

use shikumi_derivation::{
    Derivation, NarNode, OutputName, Outputs, References, Sha256, StoreDerivation, StoreDir,
    StorePath,
};
use shikumi_outputs::{ReaddressError, output_address, readdress};
use shikumi_transport::{BuildTarget, PathInfo, StoreTransport};
use tokio::io::AsyncReadExt;

use crate::report::Report;
use crate::{HarnessError, Salt};

const SH: &str = "/bin/sh";
/// the script every unit B runs: one line from its input, and the input's path, which makes the
/// input a reference of B's output.
const B: &str = "read line < \"$input\"; echo \"b $line $input\" > $out";

/// an input-addressed unit with one output. `rev` changes the derivation, and so its output
/// path, without changing what the script writes.
pub(crate) fn unit(
    dir: &StoreDir,
    system: &str,
    salt: &Salt,
    name: &str,
    rev: &str,
    script: &str,
    input: Option<&StorePath>,
) -> Result<StoreDerivation, HarnessError> {
    let mut drv = Derivation::new(
        salt.name(name)?,
        Outputs::InputAddressed(BTreeSet::from([OutputName::new("out")?])),
        system,
        SH,
    );
    drv.env("rev", rev);
    if let Some(input) = input {
        drv.input_source(input.clone())
            .env("input", dir.display(input));
    }
    drv.arg("-c").arg(script);
    Ok(dir.synthesize(drv)?)
}

pub(crate) fn out(drv: &StoreDerivation) -> &StorePath {
    drv.output("out").expect("every unit here has an out")
}

/// adds the `.drv` and builds its `out`, returning the path the daemon reports.
pub(crate) async fn realise<T: StoreTransport>(
    store: &T,
    drv: &StoreDerivation,
) -> Result<StorePath, HarnessError> {
    store.add_derivations(&[drv]).await?;
    let built = store
        .build(&[BuildTarget {
            derivation: drv.path().clone(),
            outputs: BTreeSet::from([OutputName::new("out")?]),
        }])
        .await?;
    let mut built: BTreeMap<OutputName, StorePath> = built.into_iter().next().unwrap_or_default();
    built
        .remove("out")
        .ok_or_else(|| HarnessError::Usage(format!("the build of {} reported no out", drv.path())))
}

async fn valid<T: StoreTransport>(store: &T, path: &StorePath) -> Result<bool, HarnessError> {
    Ok(store
        .query_valid(std::slice::from_ref(path))
        .await?
        .contains(path))
}

fn validity(valid: bool) -> &'static str {
    if valid { "valid" } else { "not valid" }
}

/// what B is written against: A's output re-addressed, or with re-addressing removed, the output
/// itself, which is what the cases have to catch.
struct Feed<'a, T> {
    store: &'a T,
    dir: &'a StoreDir,
    readdressing: bool,
}

impl<T: StoreTransport> Feed<'_, T> {
    /// re-addresses one output and records the NAR bytes it sent: the output's whole NAR when its
    /// address is new, nothing when it's already valid. the address is read back from the
    /// daemon, which has to hold the output's NAR hash and references there.
    async fn feed(
        &self,
        output: &StorePath,
        (case, check): (&'static str, &str),
        expect_sent: bool,
        report: &mut Report,
    ) -> Result<StorePath, HarnessError> {
        let info = path_info(self.store, output).await?;
        let expected = if expect_sent { info.nar_size } else { 0 };
        let expected = format!("{expected} bytes sent");
        if !self.readdressing {
            report.push(case, check, expected, "not re-addressed");
            return Ok(output.clone());
        }
        let readdressed = readdress(self.store, self.dir, std::slice::from_ref(output))
            .await
            .map_err(HarnessError::Readdress)?
            .remove(0);
        report.push(
            case,
            check,
            expected,
            format!("{} bytes sent", readdressed.sent),
        );
        let registered = path_info(self.store, readdressed.address.path()).await?;
        report.push(
            case,
            format!("{check}: registered nar hash"),
            info.nar_hash.to_hex(),
            registered.nar_hash.to_hex(),
        );
        report.push(
            case,
            format!("{check}: registered references"),
            joined(self.dir, &info.references),
            joined(self.dir, &registered.references),
        );
        Ok(readdressed.address.path().clone())
    }
}

async fn path_info<T: StoreTransport>(
    store: &T,
    path: &StorePath,
) -> Result<PathInfo, HarnessError> {
    Ok(store
        .query_path_info(std::slice::from_ref(path))
        .await?
        .remove(0))
}

fn joined(dir: &StoreDir, paths: &BTreeSet<StorePath>) -> String {
    if paths.is_empty() {
        return "none".to_owned();
    }
    paths
        .iter()
        .map(|path| dir.display(path))
        .collect::<Vec<_>>()
        .join(" ")
}

/// the cutoff, both ways. `readdressing` false leaves the re-add out, to show the checks fail
/// without it.
pub(crate) async fn run<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    system: &str,
    salt: &Salt,
    readdressing: bool,
    report: &mut Report,
) -> Result<(), HarnessError> {
    const SAME: &str = "early-cutoff";
    const CHANGED: &str = "early-cutoff-changed";
    let a = |rev: &str, script: &str| unit(dir, system, salt, "cutoff-a", rev, script, None);
    let b = |input: &StorePath| unit(dir, system, salt, "cutoff-b", "1", B, Some(input));
    let feed = Feed {
        store,
        dir,
        readdressing,
    };

    let a1 = a("1", "echo cutoff > $out")?;
    let a1_out = realise(store, &a1).await?;
    nar_stream(store, &a1_out, b"cutoff\n", report).await?;
    let input = feed
        .feed(&a1_out, (SAME, "first re-add"), true, report)
        .await?;
    let b1 = b(&input)?;
    let b1_out = realise(store, &b1).await?;
    // an output with a reference re-addresses with it, which is how a chain of units cuts off.
    report.push(
        SAME,
        "B output's references",
        dir.display(&input),
        joined(dir, &path_info(store, &b1_out).await?.references),
    );
    feed.feed(&b1_out, (SAME, "B's re-add"), true, report)
        .await?;

    // a changed derivation whose script writes the same bytes. its output path is new, so the
    // daemon has to build it: there's nothing cached to stand in for the rebuild.
    let a2 = a("2", "echo cutoff > $out")?;
    report.push(
        SAME,
        "rebuilt A's output before its build",
        "not valid",
        validity(valid(store, out(&a2)).await?),
    );
    let a2_out = realise(store, &a2).await?;
    report.push(
        SAME,
        "rebuilt A's output path",
        dir.display(out(&a2)),
        dir.display(&a2_out),
    );
    let same = feed
        .feed(&a2_out, (SAME, "second re-add"), false, report)
        .await?;
    report.push(
        SAME,
        "re-added path",
        dir.display(&input),
        dir.display(&same),
    );
    let b2 = b(&same)?;
    report.push(
        SAME,
        "B drv path",
        dir.display(b1.path()),
        dir.display(b2.path()),
    );
    store.add_derivations(&[&b2]).await?;
    report.push(
        SAME,
        "B output with nothing built",
        "valid",
        validity(valid(store, out(&b2)).await?),
    );
    report.push(
        SAME,
        "B output path",
        dir.display(&b1_out),
        dir.display(out(&b2)),
    );

    // changed bytes: a new address, a new B, and a B output that has to be built.
    let a3 = a("3", "echo cutoff changed > $out")?;
    let a3_out = realise(store, &a3).await?;
    let changed = feed
        .feed(&a3_out, (CHANGED, "re-add"), true, report)
        .await?;
    report.push(
        CHANGED,
        "re-added path",
        "differs from the unchanged one",
        if changed == input {
            "the unchanged one"
        } else {
            "differs from the unchanged one"
        },
    );
    let b3 = b(&changed)?;
    report.push(
        CHANGED,
        "B drv path",
        "differs from the unchanged B",
        if b3.path() == b1.path() {
            "the unchanged B"
        } else {
            "differs from the unchanged B"
        },
    );
    store.add_derivations(&[&b3]).await?;
    report.push(
        CHANGED,
        "B output before its build",
        "not valid",
        validity(valid(store, out(&b3)).await?),
    );
    let b3_out = realise(store, &b3).await?;
    report.push(
        CHANGED,
        "B output",
        dir.display(out(&b3)),
        dir.display(&b3_out),
    );
    Ok(())
}

/// reads a built output's NAR through the transport and compares it with the NAR of the bytes
/// the build wrote.
async fn nar_stream<T: StoreTransport>(
    store: &T,
    path: &StorePath,
    contents: &[u8],
    report: &mut Report,
) -> Result<(), HarnessError> {
    let info = store
        .query_path_info(std::slice::from_ref(path))
        .await?
        .remove(0);
    let mut bytes = Vec::new();
    let read = store.read_nar(&info).read_to_end(&mut bytes).await;
    let expected = NarNode::regular(false, contents.to_vec()).to_nar()?;
    report.push(
        "nar-stream",
        "sha256 of the NAR read",
        expected.hash.to_hex(),
        match read {
            Ok(_) => Sha256::digest(&bytes).to_hex(),
            Err(error) => format!("unreadable: {error}"),
        },
    );
    Ok(())
}

/// an output that names its own path can't be re-addressed. the daemon has to report the
/// self-reference, the refusal has to name the output, and neither the address a re-add that
/// dropped the self-reference would give nor a plain output refused with it may be registered.
pub(crate) async fn self_reference<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    system: &str,
    salt: &Salt,
    report: &mut Report,
) -> Result<(), HarnessError> {
    const CASE: &str = "readdress-self-reference";
    let selfish = unit(
        dir,
        system,
        salt,
        "readdress-self",
        "1",
        "echo $out > $out",
        None,
    )?;
    let built = realise(store, &selfish).await?;
    let plain = unit(
        dir,
        system,
        salt,
        "readdress-plain",
        "1",
        "echo plain > $out",
        None,
    )?;
    let plain = realise(store, &plain).await?;
    let reported = path_info(store, &built).await?;
    report.push(
        CASE,
        "the daemon's references of the output",
        "include the output",
        if reported.references.contains(&built) {
            "include the output"
        } else {
            "leave the output out"
        },
    );
    // the plain output comes first, so a refusal that added the batch's good outputs before
    // checking the rest would leave its address registered.
    let plain_info = path_info(store, &plain).await?;
    let plain_address = output_address(dir, &plain_info)?;
    let observed = match readdress(store, dir, &[plain.clone(), built.clone()]).await {
        Err(ReaddressError::SelfReference(outputs)) => {
            let error = ReaddressError::SelfReference(outputs.clone());
            eprintln!("{CASE}: refused as it should be: {error}");
            format!("refused, naming {}", outputs.join(" "))
        }
        Err(other) => return Err(HarnessError::Readdress(other)),
        Ok(readdressed) => format!("re-added as {}", readdressed[1].address.path()),
    };
    report.push(
        CASE,
        "re-add",
        format!("refused, naming {}", dir.display(&built)),
        observed,
    );
    let others = References::of(&built, reported.references.iter().cloned())
        .others()
        .clone();
    let dropped = dir.source_address(
        &built.store_name(),
        reported.nar_hash,
        References::new(others),
    )?;
    report.push(
        CASE,
        "address without the self-reference",
        "not valid",
        validity(valid(store, dropped.path()).await?),
    );
    report.push(
        CASE,
        "address of the plain output refused with it",
        "not valid",
        validity(valid(store, plain_address.path()).await?),
    );
    Ok(())
}

/// one derivation every transport's run builds under the same name, so the reports of all three
/// hold the same output path and NAR hash for it, each from a build the daemon did for that run:
/// the collection that ends each run takes the output, unrooted, before the next run.
pub(crate) async fn shared<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    system: &str,
    report: &mut Report,
) -> Result<(), HarnessError> {
    const CASE: &str = "shared-derivation";
    let drv = unit(
        dir,
        system,
        &Salt::shared(),
        "built",
        "1",
        "echo shared > $out",
        None,
    )?;
    store.add_derivations(&[&drv]).await?;
    // nothing roots it, so the collection that ended the previous run took it, and this run's
    // daemon has to build it.
    report.push(
        CASE,
        "output before its build",
        "not valid",
        validity(valid(store, out(&drv)).await?),
    );
    let built = realise(store, &drv).await?;
    report.push(CASE, "output", dir.display(out(&drv)), dir.display(&built));
    let info = store
        .query_path_info(std::slice::from_ref(&built))
        .await?
        .remove(0);
    report.push(
        CASE,
        "output nar hash",
        NarNode::regular(false, b"shared\n".to_vec())
            .to_nar()?
            .hash
            .to_hex(),
        info.nar_hash.to_hex(),
    );
    Ok(())
}
