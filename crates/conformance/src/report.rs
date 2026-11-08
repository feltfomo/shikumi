//! the checks a run produces and how they're printed: one tab-separated line per check, with the
//! value Shikumi computed beside the value the daemon reported. a note records what a daemon did
//! where Shikumi depends on no particular answer, such as whether it refuses an operation Shikumi
//! never sends, and can't fail. the report is for people reading evidence; no program reads it.

use std::fmt::Write;

use shikumi_derivation::StoreDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    pub(crate) case: &'static str,
    pub(crate) check: String,
    pub(crate) outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Shikumi's value beside the daemon's, which have to agree.
    Compared { computed: String, daemon: String },
    /// what the daemon did, with nothing to compare it to.
    Note(String),
}

impl Check {
    pub(crate) fn passed(&self) -> bool {
        match &self.outcome {
            Outcome::Compared { computed, daemon } => computed == daemon,
            Outcome::Note(_) => true,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Report {
    checks: Vec<Check>,
}

impl Report {
    pub(crate) fn push(
        &mut self,
        case: &'static str,
        check: impl Into<String>,
        computed: impl Into<String>,
        daemon: impl Into<String>,
    ) {
        self.checks.push(Check {
            case,
            check: check.into(),
            outcome: Outcome::Compared {
                computed: computed.into(),
                daemon: daemon.into(),
            },
        });
    }

    pub(crate) fn note(
        &mut self,
        case: &'static str,
        check: impl Into<String>,
        daemon: impl Into<String>,
    ) {
        self.checks.push(Check {
            case,
            check: check.into(),
            outcome: Outcome::Note(daemon.into()),
        });
    }

    #[cfg(test)]
    pub(crate) fn checks(&self) -> &[Check] {
        &self.checks
    }

    /// alters every computed value of one case, the way a drifted computation would differ, so a
    /// run can show the harness catches it. returns how many checks it altered, which is zero for a
    /// case the run doesn't have. a note has no computed value to alter.
    pub(crate) fn perturb(&mut self, case: &str, dir: &StoreDir) -> usize {
        let mut altered = 0;
        for check in self.checks.iter_mut().filter(|check| check.case == case) {
            if let Outcome::Compared { computed, .. } = &mut check.outcome {
                *computed = perturbed(computed, dir);
                altered += 1;
            }
        }
        altered
    }

    pub(crate) fn passed(&self) -> bool {
        self.checks.iter().all(Check::passed)
    }

    pub(crate) fn render(&self) -> String {
        let mut out = String::from("verdict\tcase\tcheck\tcomputed\tdaemon\n");
        let (mut compared, mut failed, mut notes) = (0, 0, 0);
        for check in &self.checks {
            let (verdict, computed, daemon) = match &check.outcome {
                Outcome::Compared { computed, daemon } => {
                    compared += 1;
                    if check.passed() {
                        ("pass", computed.as_str(), daemon)
                    } else {
                        failed += 1;
                        ("FAIL", computed.as_str(), daemon)
                    }
                }
                Outcome::Note(daemon) => {
                    notes += 1;
                    ("note", "-", daemon)
                }
            };
            writeln!(
                out,
                "{verdict}\t{}\t{}\t{}\t{}",
                check.case,
                check.check,
                computed.escape_debug(),
                daemon.escape_debug()
            )
            .unwrap();
        }
        writeln!(out, "{compared} checks, {failed} failed, {notes} notes").unwrap();
        out
    }
}

/// a store path gets one digit of its hash part changed; anything else gets a suffix.
fn perturbed(value: &str, dir: &StoreDir) -> String {
    match dir.parse(value) {
        Ok(path) => {
            let first = if path.hash_part().starts_with('0') {
                '1'
            } else {
                '0'
            };
            format!(
                "{}/{first}{}",
                dir.as_str(),
                &path.base_name()[first.len_utf8()..]
            )
        }
        Err(_) => format!("{value} (perturbed)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_perturbed_case_fails_and_the_others_still_pass() {
        let dir = StoreDir::default();
        let path = "/nix/store/0mdqa9w1p6cmli6976v4wi0sw9r4p5pr-a";
        let mut report = Report::default();
        report.push("one", "path", path, path);
        report.push("two", "hash", "abc", "abc");
        assert!(report.passed());
        assert_eq!(report.perturb("one", &dir), 1);
        assert_eq!(report.perturb("missing", &dir), 0);
        assert!(!report.checks()[0].passed());
        let Outcome::Compared { computed, .. } = &report.checks()[0].outcome else {
            panic!("a pushed check is compared");
        };
        assert!(dir.parse(computed).is_ok());
        assert!(report.checks()[1].passed());
        assert!(!report.passed());
        assert!(report.render().contains("FAIL\tone\tpath"));
    }

    #[test]
    fn a_note_never_fails_and_isnt_perturbed() {
        let mut report = Report::default();
        report.note("probe", "answer", "refused: not privileged");
        assert_eq!(report.perturb("probe", &StoreDir::default()), 0);
        assert!(report.passed());
        assert!(report.render().contains("note\tprobe\tanswer"));
        assert!(report.render().contains("0 checks, 0 failed, 1 notes"));
    }

    #[test]
    fn a_perturbed_value_that_isnt_a_path_still_differs() {
        assert_ne!(perturbed("refused", &StoreDir::default()), "refused");
    }
}
