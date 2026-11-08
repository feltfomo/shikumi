//! per-step timing inside one measured build. a record's wall time says how long the build took;
//! its buckets say where that time went, one bucket per kind of step the executor paid for. the
//! timeline that fills them is anchored at the instant the record's timer started, so a step that
//! ran before the timer or after it shows up as a span outside the build instead of vanishing.

use std::collections::BTreeMap;
use std::sync::Mutex;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::contract::ContractError;
use crate::measure::monotonic_ns;

/// a kind of step an executor pays for in a build. each is its own bucket, so a cost the product
/// would pay can't hide inside another step or outside the timer.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Step {
    /// reading and writing the executor's own record of each unit's source and output, which a
    /// no-op and an edit start from.
    State,
    /// opening the executor's connections to the store, which every command pays again.
    Connect,
    /// finding which unit sources changed: stats, reads and NAR hashing of the working tree.
    SourceScan,
    /// adding changed unit sources to the store as content-addressed paths.
    SourceAdd,
    /// asking the store which paths are already valid.
    ValidityQuery,
    /// computing `.drv` contents and paths in memory.
    DrvSynthesis,
    /// sending `.drv` files to the store.
    DrvWrite,
    /// a build request to the daemon, from send to reply: its sandbox setup and the builder
    /// itself are inside, because the daemon reports neither apart.
    Build,
    /// re-adding built outputs at the content-addressed path their bytes give.
    Readdress,
    /// adding outputs an executor produced itself to the store by content.
    OutputAdd,
    /// writing the root manifest and registering its root.
    Rooting,
    /// setting up a sandbox the executor owns: namespaces, mounts and identity maps.
    SandboxSetup,
    /// creating the process a unit runs in, up to the point its sandbox setup starts.
    ProcessStart,
    /// running the compiler or builder itself, from exec to exit.
    Compile,
    /// a unit's warm incremental state outside the compile: recording the session it starts
    /// from, counting what rustc reused, and canonicalizing the rlib an incremental build writes.
    IncrementalState,
    /// fetching paths from a binary cache into the store.
    Substitution,
    /// the daemon's garbage collection.
    GarbageCollection,
}

/// the time one kind of step took inside a measured build. offsets are from the instant the
/// build's timer started; when steps ran concurrently, `total_ns` sums them and can exceed the
/// span they cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bucket {
    pub step: Step,
    /// how many times the step ran.
    pub count: u32,
    /// the summed duration of every run of the step, in nanoseconds.
    pub total_ns: u64,
    /// when the first run of the step started, in nanoseconds after the timer started.
    pub first_start_ns: u64,
    /// when the last run of the step ended, in nanoseconds after the timer started.
    pub last_end_ns: u64,
}

impl Bucket {
    /// the rules a bucket holds against the build it belongs to. a span outside the build's wall
    /// time is a step the timer didn't cover, and a total that no run count could give is a
    /// bucket that wasn't measured.
    pub fn check(&self, wall_time_ns: u64) -> Result<(), ContractError> {
        let fail = |detail: String| Err(ContractError(format!("bucket {:?} {detail}", self.step)));
        if self.count == 0 {
            return fail("ran zero times".to_owned());
        }
        if self.first_start_ns > self.last_end_ns {
            return fail("ends before it starts".to_owned());
        }
        if self.last_end_ns > wall_time_ns {
            return fail(format!(
                "ends at {} ns, after the build's {wall_time_ns} ns: the step ran outside the timer",
                self.last_end_ns
            ));
        }
        let span = self.last_end_ns - self.first_start_ns;
        if self.total_ns > span.saturating_mul(u64::from(self.count)) {
            return fail(format!(
                "sums to {} ns over {} runs inside a {span} ns span",
                self.total_ns, self.count
            ));
        }
        Ok(())
    }
}

/// collects the steps of one measured build from any thread, against a start instant taken on
/// the monotonic clock. a step that started before the start is kept as an error rather than
/// clamped, since clamping it would hide exactly the bookkeeping a timer must not miss.
#[derive(Debug)]
pub struct Timeline {
    start_ns: u64,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    buckets: BTreeMap<Step, Bucket>,
    /// every run's span, so the time no bucket covered can be told from the wall time.
    spans: Vec<(u64, u64)>,
    before_start: Option<Step>,
}

/// a finished timeline: its buckets, and how much of the build at least one of them covered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Steps {
    pub buckets: Vec<Bucket>,
    pub covered_ns: u64,
}

impl Timeline {
    /// starts the timeline now.
    pub fn start() -> Self {
        Self::starting_at(monotonic_ns())
    }

    /// starts the timeline at a monotonic-clock reading taken earlier, such as the one a record's
    /// timer took.
    pub fn starting_at(start_ns: u64) -> Self {
        Self {
            start_ns,
            state: Mutex::default(),
        }
    }

    /// records one run of `step` between two monotonic-clock readings. readings can come from
    /// another process on this machine, since the clock is shared.
    pub fn record(&self, step: Step, start_ns: u64, end_ns: u64) {
        let mut state = self
            .state
            .lock()
            .expect("a timeline's lock is never poisoned");
        if start_ns < self.start_ns {
            state.before_start.get_or_insert(step);
            return;
        }
        let first = start_ns - self.start_ns;
        let last = end_ns.max(start_ns) - self.start_ns;
        state.spans.push((first, last));
        let bucket = state.buckets.entry(step).or_insert(Bucket {
            step,
            count: 0,
            total_ns: 0,
            first_start_ns: first,
            last_end_ns: last,
        });
        bucket.count += 1;
        bucket.total_ns += last - first;
        bucket.first_start_ns = bucket.first_start_ns.min(first);
        bucket.last_end_ns = bucket.last_end_ns.max(last);
    }

    /// times `run` as one run of `step`.
    pub fn time<T>(&self, step: Step, run: impl FnOnce() -> T) -> T {
        let start = monotonic_ns();
        let value = run();
        self.record(step, start, monotonic_ns());
        value
    }

    /// times a future as one run of `step`, from its first poll to its completion.
    pub async fn time_async<T>(&self, step: Step, run: impl Future<Output = T>) -> T {
        let start = monotonic_ns();
        let value = run.await;
        self.record(step, start, monotonic_ns());
        value
    }

    /// the buckets in step order with the time they cover, or the first step recorded as
    /// starting before the timeline.
    pub fn finish(self) -> Result<Steps, ContractError> {
        let mut state = self
            .state
            .into_inner()
            .expect("a timeline's lock is never poisoned");
        if let Some(step) = state.before_start {
            return Err(ContractError(format!(
                "bucket {step:?} started before the build's timer: the step ran outside it"
            )));
        }
        state.spans.sort_unstable();
        let mut covered_ns = 0;
        let mut reach = 0;
        for (start, end) in state.spans {
            let start = start.max(reach);
            if end > start {
                covered_ns += end - start;
                reach = end;
            }
        }
        Ok(Steps {
            buckets: state.buckets.into_values().collect(),
            covered_ns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_of_one_step_merge_into_one_bucket() {
        let timeline = Timeline::starting_at(1_000);
        timeline.record(Step::Build, 1_100, 1_400);
        timeline.record(Step::Build, 1_200, 1_300);
        timeline.record(Step::DrvWrite, 1_050, 1_060);
        let steps = timeline.finish().unwrap();
        // 1050..1060 and 1100..1400 cover 310 ns; the run inside 1200..1300 adds nothing.
        assert_eq!(steps.covered_ns, 310);
        let buckets = steps.buckets;
        assert_eq!(
            buckets,
            [
                Bucket {
                    step: Step::DrvWrite,
                    count: 1,
                    total_ns: 10,
                    first_start_ns: 50,
                    last_end_ns: 60,
                },
                Bucket {
                    step: Step::Build,
                    count: 2,
                    total_ns: 400,
                    first_start_ns: 100,
                    last_end_ns: 400,
                },
            ]
        );
        assert!(buckets.iter().all(|b| b.check(400).is_ok()));
    }

    // the two ways a step escapes its timer: it ran before the timer started, which the timeline
    // refuses, or after it stopped, which the bucket's own check refuses against the wall time.
    #[test]
    fn a_step_outside_the_timer_is_refused_either_side() {
        let timeline = Timeline::starting_at(1_000);
        timeline.record(Step::Readdress, 900, 1_100);
        let error = timeline.finish().unwrap_err().to_string();
        assert!(error.contains("before the build's timer"), "{error}");

        let timeline = Timeline::starting_at(1_000);
        timeline.record(Step::Readdress, 1_100, 2_500);
        let bucket = timeline.finish().unwrap().buckets[0];
        let error = bucket.check(1_000).unwrap_err().to_string();
        assert!(error.contains("outside the timer"), "{error}");
    }

    #[test]
    fn a_bucket_no_run_count_could_give_is_refused() {
        let bucket = Bucket {
            step: Step::Compile,
            count: 2,
            total_ns: 500,
            first_start_ns: 0,
            last_end_ns: 100,
        };
        assert!(bucket.check(1_000).is_err());
        assert!(Bucket { count: 0, ..bucket }.check(1_000).is_err());
        assert!(
            Bucket {
                first_start_ns: 200,
                ..bucket
            }
            .check(1_000)
            .is_err()
        );
        assert!(
            Bucket {
                total_ns: 150,
                ..bucket
            }
            .check(1_000)
            .is_ok()
        );
    }
}
