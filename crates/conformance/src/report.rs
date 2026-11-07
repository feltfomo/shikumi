//! the checks a run produces and how they're printed: one tab-separated line per check, with the
//! value Shikumi computed beside the value the daemon reported. the report is for people reading
//! evidence; no program reads it.

use std::fmt::Write;

use shikumi_derivation::StoreDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    pub(crate) case: &'static str,
    pub(crate) check: String,
    pub(crate) computed: String,
    pub(crate) daemon: String,
}

impl Check {
    pub(crate) fn passed(&self) -> bool {
        self.computed == self.daemon
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
            computed: computed.into(),
            daemon: daemon.into(),
        });
    }

    #[cfg(test)]
    pub(crate) fn checks(&self) -> &[Check] {
        &self.checks
    }

    /// alters every computed value of one case, the way a drifted computation would differ, so a
    /// run can show the harness catches it. returns how many checks it altered, which is zero for a
    /// case the run doesn't have.
    pub(crate) fn perturb(&mut self, case: &str, dir: &StoreDir) -> usize {
        let mut altered = 0;
        for check in self.checks.iter_mut().filter(|check| check.case == case) {
            check.computed = perturbed(&check.computed, dir);
            altered += 1;
        }
        altered
    }

    pub(crate) fn passed(&self) -> bool {
        self.checks.iter().all(Check::passed)
    }

    pub(crate) fn render(&self) -> String {
        let mut out = String::from("verdict\tcase\tcheck\tcomputed\tdaemon\n");
        for check in &self.checks {
            let verdict = if check.passed() { "pass" } else { "FAIL" };
            writeln!(
                out,
                "{verdict}\t{}\t{}\t{}\t{}",
                check.case,
                check.check,
                check.computed.escape_debug(),
                check.daemon.escape_debug()
            )
            .unwrap();
        }
        let failed = self.checks.iter().filter(|check| !check.passed()).count();
        writeln!(out, "{} checks, {failed} failed", self.checks.len()).unwrap();
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
        assert!(dir.parse(&report.checks()[0].computed).is_ok());
        assert!(report.checks()[1].passed());
        assert!(!report.passed());
        assert!(report.render().contains("FAIL\tone\tpath"));
    }

    #[test]
    fn a_perturbed_value_that_isnt_a_path_still_differs() {
        assert_ne!(perturbed("refused", &StoreDir::default()), "refused");
    }
}
