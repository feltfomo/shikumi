//! the result record: one measured build, written as one JSON line. every executor's measurements
//! use this type, so comparing a candidate with the Cargo baseline is a join on these fields.
//!
//! a record carries its own proof that it measured what it's named for. its buckets have to cover
//! every step its mechanism pays in its scenario, inside the timer; its outcome has to show the
//! work the fixture's model predicts, and, where there's a reference, outputs identical to it. a
//! record that fails any of these can't be made, by a writer or by a reader, so a records file
//! that validates holds only rows that are what they say.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU32;

use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};

use crate::contract::{ContractError, Identifier, Sha256, Text, Version};
use crate::fixture::overhead::{Shape, UnitGraph};
use crate::fixture::rust_workspace::{Edit, RustWorkspace, RustWorkspaceParams, Tracking};
use crate::steps::{Bucket, Step};

pub type RecordVersion = Version<2>;

/// one measured build whose buckets, outcome and fixture agree with each other. it holds the
/// record as written, checked, so writing it out never regenerates or re-checks anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "RawRecord", into = "RawRecord")]
pub struct Record {
    raw: RawRecord,
}

/// what a writer fills to make a record. the unit count and graph depth aren't here: they come
/// from the fixture, so a record built in code can't claim a different graph than it names.
#[derive(Debug, Clone)]
pub struct RecordParts {
    pub started_at_unix_ms: u64,
    pub machine: Machine,
    pub nix: Option<NixImplementation>,
    pub candidate: Candidate,
    pub transport: Option<Transport>,
    pub fixture: Fixture,
    pub scenario: Scenario,
    pub incremental: Option<bool>,
    pub iteration: u32,
    pub wall_time_ns: u64,
    pub cpu: Cpu,
    pub buckets: Vec<Bucket>,
    pub covered_ns: u64,
    pub outcome: Outcome,
    pub store_valid_paths: Option<u64>,
}

/// the record as written. it owns the published schema and its field descriptions; [`Record`]
/// adds the rules that span fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    title = "Shikumi benchmark record",
    transform = crate::contract::require_all_properties
)]
struct RawRecord {
    schema_version: RecordVersion,
    /// when the measured build started, in milliseconds since the Unix epoch.
    started_at_unix_ms: u64,
    machine: Machine,
    /// the Nix implementation the candidate ran against; null when no Nix is in play, as for the
    /// Cargo baseline.
    #[serde(deserialize_with = "Option::deserialize")]
    nix: Option<NixImplementation>,
    candidate: Candidate,
    /// how work and outputs move between the candidate and the store; null when the candidate
    /// has no store transport.
    #[serde(deserialize_with = "Option::deserialize")]
    transport: Option<Transport>,
    fixture: Fixture,
    scenario: Scenario,
    /// whether rustc's incremental compilation was on; null for fixtures that run no compiler.
    #[serde(deserialize_with = "Option::deserialize")]
    incremental: Option<bool>,
    /// the units the fixture holds: crates for a Rust workspace, units for an overhead graph. it
    /// must match the fixture.
    unit_count: u32,
    /// the fixture's longest dependency chain, in edges. it must match the fixture.
    graph_depth: u32,
    /// which repetition of the scenario this is, from 0.
    iteration: u32,
    /// elapsed wall-clock time of the measured build, in nanoseconds.
    wall_time_ns: u64,
    cpu: Cpu,
    /// where the wall time went, one bucket per kind of step, in step order. every step the
    /// candidate's mechanism pays in the scenario has a bucket.
    buckets: Vec<Bucket>,
    /// the wall time at least one bucket's run covered, in nanoseconds; the rest went to work no
    /// bucket names, such as scheduling and waiting.
    covered_ns: u64,
    outcome: Outcome,
    /// how many valid paths the store held when the build started; null when it wasn't read.
    #[serde(deserialize_with = "Option::deserialize")]
    store_valid_paths: Option<u64>,
}

impl TryFrom<RawRecord> for Record {
    type Error = ContractError;

    fn try_from(raw: RawRecord) -> Result<Self, Self::Error> {
        let model = raw.fixture.model()?;
        if raw.unit_count != model.graph.units {
            return Err(ContractError(format!(
                "unit_count {} doesn't match the fixture's {} units",
                raw.unit_count, model.graph.units
            )));
        }
        if raw.graph_depth != model.graph.depth {
            return Err(ContractError(format!(
                "graph_depth {} doesn't match the fixture's depth {}",
                raw.graph_depth, model.graph.depth
            )));
        }
        check(&raw, &model)?;
        Ok(Self { raw })
    }
}

impl From<Record> for RawRecord {
    fn from(record: Record) -> Self {
        record.raw
    }
}

/// every rule a record holds, against its fixture's model, generated once.
fn check(raw: &RawRecord, model: &FixtureModel) -> Result<(), ContractError> {
    raw.scenario.fits(&raw.fixture)?;
    check_context(raw)?;
    check_buckets(raw)?;
    raw.outcome.check(raw, model)
}

/// the wording every rule a record breaks is reported in.
pub fn unmet(scenario: Scenario, detail: impl fmt::Display) -> String {
    format!("the {scenario:?} scenario didn't measure what it claims: {detail}")
}

fn refuse(scenario: Scenario, detail: impl fmt::Display) -> ContractError {
    ContractError(unmet(scenario, detail))
}

/// each step at most once, every one inside the timer, and every step the mechanism pays in the
/// scenario present: a missing bucket is a step the record's timer didn't measure.
fn check_buckets(raw: &RawRecord) -> Result<(), ContractError> {
    // a step's discriminant indexes a bit, so a record's buckets are checked without allocating.
    let mut seen = 0u64;
    for bucket in &raw.buckets {
        let bit = 1u64 << (bucket.step as u32);
        if seen & bit != 0 {
            return Err(refuse(
                raw.scenario,
                format!("bucket {:?} appears twice", bucket.step),
            ));
        }
        seen |= bit;
        bucket.check(raw.wall_time_ns)?;
    }
    let mechanism = raw.candidate.mechanism;
    let required = mechanism
        .required_steps(raw.scenario, raw.incremental == Some(true))
        .ok_or_else(|| {
            refuse(
                raw.scenario,
                format!("mechanism {mechanism:?} has no such scenario"),
            )
        })?;
    let missing: Vec<&Step> = required
        .iter()
        .filter(|s| seen & (1u64 << (**s as u32)) == 0)
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(refuse(
            raw.scenario,
            format!(
                "{mechanism:?} pays for {missing:?}, which have no bucket: the timer didn't measure them"
            ),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = crate::contract::require_all_properties)]
pub struct Machine {
    /// the host's name, or the name given on the command line.
    pub name: Text,
    /// the CPU model string; null where the platform doesn't report one.
    #[serde(deserialize_with = "Option::deserialize")]
    pub cpu_model: Option<Text>,
    pub logical_cpus: u32,
    /// Rust's name for the operating system, such as `linux` or `macos`.
    pub os: Text,
    /// Rust's name for the CPU architecture, such as `x86_64` or `aarch64`.
    pub arch: Text,
}

/// the Nix a candidate ran against, with what distinguishes two runs of the same version: where it
/// ran, the settings that change what a build costs, and whether the daemon trusted the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NixImplementation {
    pub implementation: NixFlavor,
    /// the version the implementation reports, such as `2.95.4`.
    pub version: Text,
    pub environment: NixEnvironment,
    pub settings: NixSettings,
    pub client_trust: ClientTrust,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum NixFlavor {
    Cppnix,
    Determinate,
    Lix,
}

/// where the Nix implementation ran, relative to the machine the record names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum NixEnvironment {
    /// directly on the machine's own operating system.
    Host,
    /// inside a virtual machine on that machine.
    VirtualMachine,
    /// inside a container on that machine.
    Container,
}

/// the settings that change what a build costs, as the implementation reported them for the run.
/// each is read rather than assumed: a run whose settings weren't read can't be told apart from
/// another, so the contract has no "unknown" value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NixSettings {
    pub sandbox: Setting<SandboxMode>,
    /// whether builds run in their own cgroups.
    pub use_cgroups: Setting<bool>,
    pub max_jobs: Setting<u32>,
    /// cores offered to each build; 0 is Nix's own value for all of them.
    pub cores: Setting<u32>,
}

/// a setting's value for a run, or the fact that the implementation has no such setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", content = "value", rename_all = "kebab-case")]
#[schemars(rename = "Setting_{T}")]
pub enum Setting<T> {
    Set(T),
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    Enabled,
    Disabled,
    Relaxed,
}

/// what the daemon said about the client. a trusted client may override settings per connection,
/// so the record says which kind it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ClientTrust {
    Trusted,
    Untrusted,
    /// the daemon didn't say, which older protocol versions and the CLI don't.
    Unknown,
}

/// the executor being measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub name: Identifier,
    /// the version line the candidate's tool reports.
    pub version: Text,
    pub mechanism: Mechanism,
}

/// how a candidate runs a unit, which decides the steps it pays for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Mechanism {
    /// plain Cargo, in place, with its own target directory.
    Cargo,
    /// one derivation per unit, built by the daemon in its sandbox.
    DerivationPerUnit,
    /// one derivation per batch of ready units, with one output per unit.
    BatchedDerivations,
    /// the executor runs the compiler in a sandbox it owns and adds outputs by content.
    OwnedSandbox,
    /// the store's own operations, which no executor changes: collection and registration.
    Store,
}

impl Mechanism {
    /// the steps the mechanism pays for in the scenario, each of which needs a bucket, with
    /// seeded units' warm state among them; `None` when the mechanism has no such scenario. it
    /// matches every pair, so a new mechanism or scenario doesn't build until it's placed here.
    pub(crate) fn required_steps(
        self,
        scenario: Scenario,
        seeded: bool,
    ) -> Option<&'static [Step]> {
        use Kind as K;
        use Step::*;
        const DAEMON_BUILD: &[Step] = &[
            State,
            Connect,
            SourceScan,
            SourceAdd,
            ValidityQuery,
            DrvSynthesis,
            DrvWrite,
            Build,
            Readdress,
            Rooting,
        ];
        const DAEMON_DO_NOTHING: &[Step] =
            &[Connect, DrvSynthesis, DrvWrite, Build, Readdress, Rooting];
        const OWNED_BUILD: &[Step] = &[
            State,
            Connect,
            SourceScan,
            ValidityQuery,
            ProcessStart,
            SandboxSetup,
            Compile,
            OutputAdd,
            Rooting,
        ];
        const OWNED_SEEDED_BUILD: &[Step] = &[
            State,
            Connect,
            SourceScan,
            ValidityQuery,
            ProcessStart,
            SandboxSetup,
            Compile,
            IncrementalState,
            OutputAdd,
            Rooting,
        ];
        const OWNED_DO_NOTHING: &[Step] = &[
            Connect,
            ProcessStart,
            SandboxSetup,
            Compile,
            OutputAdd,
            Rooting,
        ];
        const NO_OP: &[Step] = &[State, Connect, SourceScan, ValidityQuery];
        match (self, scenario.kind()) {
            (Self::Cargo, K::RustBuild(_) | K::RustNoOp) => Some(&[Compile]),
            (Self::Cargo, K::DoNothing(_) | K::Substitution | K::Collection | K::Registration) => {
                None
            }
            (Self::DerivationPerUnit | Self::BatchedDerivations, K::RustNoOp) => Some(NO_OP),
            (Self::DerivationPerUnit | Self::BatchedDerivations, K::RustBuild(_)) => {
                Some(DAEMON_BUILD)
            }
            (Self::DerivationPerUnit | Self::BatchedDerivations, K::DoNothing(_)) => {
                Some(DAEMON_DO_NOTHING)
            }
            (Self::DerivationPerUnit | Self::BatchedDerivations, K::Substitution) => {
                Some(&[ValidityQuery, Step::Substitution])
            }
            (
                Self::DerivationPerUnit | Self::BatchedDerivations,
                K::Collection | K::Registration,
            ) => None,
            (Self::OwnedSandbox, K::RustNoOp) => Some(NO_OP),
            (Self::OwnedSandbox, K::RustBuild(_)) if seeded => Some(OWNED_SEEDED_BUILD),
            (Self::OwnedSandbox, K::RustBuild(_)) => Some(OWNED_BUILD),
            (Self::OwnedSandbox, K::DoNothing(_)) => Some(OWNED_DO_NOTHING),
            (Self::OwnedSandbox, K::Substitution | K::Collection | K::Registration) => None,
            (Self::Store, K::Collection) => Some(&[GarbageCollection]),
            (Self::Store, K::Registration) => Some(&[SourceAdd]),
            (Self::Store, K::RustBuild(_) | K::RustNoOp | K::DoNothing(_) | K::Substitution) => {
                None
            }
        }
    }

    /// what makes one of its units rebuild.
    fn tracking(self) -> Tracking {
        match self {
            Self::Cargo => Tracking::ReadFiles,
            Self::DerivationPerUnit
            | Self::BatchedDerivations
            | Self::OwnedSandbox
            | Self::Store => Tracking::UnitOutputs,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// the worker protocol over the local daemon's unix socket.
    DaemonSocket,
    /// the Nix command-line tools.
    Cli,
    /// the worker protocol to `nix-daemon --stdio` over ssh.
    SshNg,
}

/// whose CPU time a record counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CpuScope {
    /// the measured process and every child it waited for.
    ProcessTree,
    /// every CPU of the machine, which is the only scope that sees a daemon's work; it includes
    /// whatever else ran meanwhile.
    Machine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cpu {
    pub scope: CpuScope,
    /// user CPU time over the build, in nanoseconds.
    pub user_ns: u64,
    /// system CPU time over the build, in nanoseconds.
    pub system_ns: u64,
}

/// what was built, with enough to regenerate it and a digest to prove the regeneration matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Fixture {
    RustWorkspace {
        seed: u64,
        parameters: RustWorkspaceParams,
        /// the digest of the generated tree before any scenario edit.
        digest: Sha256,
    },
    Overhead {
        shape: Shape,
        units: NonZeroU32,
        /// the digest of the unit graph's canonical JSON.
        digest: Sha256,
    },
}

/// the size of a fixture's unit graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphFacts {
    pub units: u32,
    pub depth: u32,
}

/// a fixture's model, generated once for every rule a record checks against it: its graph, and
/// for a Rust workspace the workspace itself, which knows what each edit rebuilds.
#[derive(Debug)]
pub struct FixtureModel {
    pub graph: GraphFacts,
    workspace: Option<RustWorkspace>,
}

/// what identifies a fixture's bytes: everything but the digest a record claims for them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FixtureKey {
    RustWorkspace(u64, RustWorkspaceParams),
    Overhead(Shape, NonZeroU32),
}

impl Fixture {
    /// the fixture's model. a Rust workspace's depth depends on where the seed puts the large
    /// crate, so its model is generated, without rendering a file; generating it also checks the
    /// parameters' rules.
    pub fn model(&self) -> Result<FixtureModel, ContractError> {
        match self {
            Self::RustWorkspace {
                seed, parameters, ..
            } => {
                let workspace = RustWorkspace::generate(*seed, parameters.clone())?;
                Ok(FixtureModel {
                    graph: GraphFacts {
                        units: parameters.unit_count(),
                        depth: workspace.graph_depth(),
                    },
                    workspace: Some(workspace),
                })
            }
            Self::Overhead { shape, units, .. } => Ok(FixtureModel {
                graph: GraphFacts {
                    units: units.get(),
                    depth: shape.depth(*units),
                },
                workspace: None,
            }),
        }
    }

    /// the unit count and depth, from the fixture's model.
    pub fn graph(&self) -> Result<GraphFacts, ContractError> {
        Ok(self.model()?.graph)
    }

    fn key(&self) -> FixtureKey {
        match self {
            Self::RustWorkspace {
                seed, parameters, ..
            } => FixtureKey::RustWorkspace(*seed, parameters.clone()),
            Self::Overhead { shape, units, .. } => FixtureKey::Overhead(*shape, *units),
        }
    }

    /// the digest of the fixture regenerated from its seed and parameters.
    fn regenerated_digest(&self) -> Result<Sha256, ContractError> {
        match self {
            Self::RustWorkspace {
                seed, parameters, ..
            } => Ok(RustWorkspace::generate(*seed, parameters.clone())?
                .render()
                .digest()),
            Self::Overhead { shape, units, .. } => Ok(UnitGraph::generate(*shape, *units).digest()),
        }
    }

    pub fn digest(&self) -> Sha256 {
        match self {
            Self::RustWorkspace { digest, .. } | Self::Overhead { digest, .. } => *digest,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Scenario {
    /// a build from an empty target directory, or of units none of which are in the store.
    Cold,
    /// a build straight after a successful one, with nothing changed.
    NoOp,
    /// a build after one private function's body changed.
    OneFunctionEdit,
    /// a build after a public signature changed in a crate many others depend on.
    PublicSignatureEdit,
    /// a build after an edit to the hub's crate directory that rustc never reads, so the rebuilt
    /// unit's output is byte-identical and everything downstream has to be skipped.
    IdenticalOutputEdit,
    /// a wide graph of do-nothing units, every one built: the fixed cost an executor adds per unit.
    UnitOverhead,
    /// a chain of do-nothing units, every one built in turn: the latency an executor adds per
    /// level of depth.
    LatencyFloor,
    /// fetching every unit's sources and outputs, through their root manifest, from a local
    /// binary cache into a store that has none of them.
    Substitution,
    /// collecting garbage in a store where rooted manifests keep some unit paths and the rest
    /// are unrooted.
    GarbageCollection,
    /// adding a batch of new content-addressed paths to the store.
    PathRegistration,
}

/// what kind of measurement a scenario is, decided once per scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// a build of the Rust fixture: cold, or after an edit.
    RustBuild(Option<Edit>),
    /// a build of the Rust fixture with nothing changed.
    RustNoOp,
    /// every unit of a do-nothing graph of this shape, built.
    DoNothing(Shape),
    Substitution,
    Collection,
    Registration,
}

impl Scenario {
    fn kind(self) -> Kind {
        match self {
            Self::Cold => Kind::RustBuild(None),
            Self::NoOp => Kind::RustNoOp,
            Self::OneFunctionEdit => Kind::RustBuild(Some(Edit::OneFunction)),
            Self::PublicSignatureEdit => Kind::RustBuild(Some(Edit::PublicSignature)),
            Self::IdenticalOutputEdit => Kind::RustBuild(Some(Edit::IdenticalOutput)),
            Self::UnitOverhead => Kind::DoNothing(Shape::Wide),
            Self::LatencyFloor => Kind::DoNothing(Shape::Chain),
            Self::Substitution => Kind::Substitution,
            Self::GarbageCollection => Kind::Collection,
            Self::PathRegistration => Kind::Registration,
        }
    }

    /// the source change the scenario applies before it builds, if any.
    pub fn edit(self) -> Option<Edit> {
        match self.kind() {
            Kind::RustBuild(edit) => edit,
            _ => None,
        }
    }

    /// whether the scenario runs rustc, and so has to say whether its units were incremental.
    fn compiles(self) -> bool {
        matches!(self.kind(), Kind::RustBuild(_) | Kind::RustNoOp)
    }

    /// whether every output an executor builds in this scenario is new, so the build has to send
    /// output bytes; `Some(false)` when nothing new can be produced, so it has to send none.
    fn new_outputs(self) -> Option<bool> {
        match self.kind() {
            Kind::RustBuild(Some(Edit::IdenticalOutput)) | Kind::RustNoOp => Some(false),
            Kind::RustBuild(_) | Kind::DoNothing(_) => Some(true),
            Kind::Substitution | Kind::Collection | Kind::Registration => None,
        }
    }

    /// whether the scenario can be run on the fixture.
    fn fits(self, fixture: &Fixture) -> Result<(), ContractError> {
        let fits = match (self.kind(), fixture) {
            (
                Kind::RustBuild(_) | Kind::RustNoOp | Kind::Substitution,
                Fixture::RustWorkspace { .. },
            ) => true,
            (Kind::DoNothing(wanted), Fixture::Overhead { shape, .. }) => *shape == wanted,
            (Kind::Collection | Kind::Registration, Fixture::Overhead { shape, .. }) => {
                *shape == Shape::Wide
            }
            _ => false,
        };
        if fits {
            Ok(())
        } else {
            Err(refuse(self, "it can't run on this fixture"))
        }
    }

    /// how many units a build in this scenario must compile, by the fixture's model; `None` when
    /// the scenario isn't a build.
    pub fn expected_built(self, mechanism: Mechanism, model: &FixtureModel) -> Option<u32> {
        match self.kind() {
            Kind::RustBuild(None) | Kind::DoNothing(_) => Some(model.graph.units),
            Kind::RustNoOp => Some(0),
            Kind::RustBuild(Some(edit)) => Some(
                model
                    .workspace
                    .as_ref()?
                    .units_rebuilt_by(edit, mechanism.tracking()),
            ),
            Kind::Substitution | Kind::Collection | Kind::Registration => None,
        }
    }
}

/// the check a timed build has to pass: it did the work its row claims, and no more or less.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(transform = crate::contract::require_all_variant_properties)]
pub enum Outcome {
    Build {
        /// units whose compiler or builder ran.
        built: u32,
        /// units found up to date without running anything.
        skipped: u32,
        /// units the fixture's model says the scenario has to build.
        expected_built: u32,
        /// NAR bytes of unit sources sent to the store.
        source_bytes_sent: u64,
        /// NAR bytes of unit outputs sent to the store, by re-addressing or adding them.
        output_bytes_sent: u64,
        /// the identity of every unit's output after the build, against a reference; null where
        /// no reference exists, as for Cargo, whose outputs no other candidate produces.
        #[serde(deserialize_with = "Option::deserialize")]
        identity: Option<Identity>,
        /// what an executor's incremental units started from; null when its units compile
        /// without incremental state, and for Cargo, whose session is its own target directory.
        #[serde(deserialize_with = "Option::deserialize")]
        warm: Option<Warm>,
    },
    Substitution {
        /// paths the store held after the substitution, out of those it was asked for.
        substituted: u32,
        expected: u32,
        /// NAR bytes received from the cache.
        bytes_received: u64,
    },
    Collection {
        /// rooted paths still valid after the collection.
        kept: u32,
        expected_kept: u32,
        /// unrooted unit paths no longer valid after the collection.
        deleted: u32,
        expected_deleted: u32,
    },
    Registration {
        /// paths valid after the add, out of those sent.
        registered: u32,
        expected: u32,
    },
}

/// the warm state an incremental build's units carried in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Warm {
    /// built units that started from a session of an earlier build.
    pub seeded: u32,
    /// object work products in the sessions the built units finalized.
    pub work_products: u32,
    /// how many of those rustc took from the seeded session instead of rebuilding.
    pub reused: u32,
}

/// a digest over each unit's name and output NAR hash, in unit order, beside the digest a
/// reference build of the same fixture state gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub outputs: Sha256,
    pub reference: Sha256,
}

impl Outcome {
    fn check(&self, raw: &RawRecord, model: &FixtureModel) -> Result<(), ContractError> {
        let scenario = raw.scenario;
        let fail = |detail: String| Err(refuse(scenario, detail));
        let units = model.graph.units;
        match self {
            Self::Build {
                built,
                skipped,
                expected_built,
                source_bytes_sent,
                output_bytes_sent,
                identity,
                warm,
            } => {
                let Some(expected) = scenario.expected_built(raw.candidate.mechanism, model) else {
                    return fail("its outcome is of another kind".to_owned());
                };
                if *expected_built != expected {
                    return fail(format!(
                        "it expects {expected_built} units built where the fixture's model gives {expected}"
                    ));
                }
                if built != expected_built {
                    return fail(format!("{built} units built, {expected_built} expected"));
                }
                if built + skipped != units {
                    return fail(format!(
                        "{built} built and {skipped} skipped don't make the fixture's {units} units"
                    ));
                }
                if let Some(identity) = identity
                    && identity.outputs != identity.reference
                {
                    return fail(format!(
                        "its outputs digest {} differs from the reference {}",
                        identity.outputs, identity.reference
                    ));
                }
                let mechanism = raw.candidate.mechanism;
                let executor = mechanism != Mechanism::Cargo;
                // what a build sends says what it built: a no-op and a cutoff move no output bytes,
                // and a build whose every output is new and sent none found them in the store, a
                // warm store standing in for a cold one. Cargo sends nothing either way.
                match scenario.new_outputs() {
                    Some(false) if *output_bytes_sent != 0 => {
                        return fail(format!(
                            "{output_bytes_sent} output bytes sent where nothing new can be produced"
                        ));
                    }
                    Some(true) if executor && *output_bytes_sent == 0 => {
                        return fail(
                            "no output bytes sent where every built output is new".to_owned(),
                        );
                    }
                    _ => {}
                }
                if scenario == Scenario::NoOp && *source_bytes_sent != 0 {
                    return fail(format!(
                        "{source_bytes_sent} source bytes sent with nothing changed"
                    ));
                }
                if !executor {
                    return match warm {
                        None => Ok(()),
                        Some(_) => fail(
                            "Cargo's session is its target directory, not warm state".to_owned(),
                        ),
                    };
                }
                check_warm(raw, *built, warm.as_ref())?;
                // an executor's outputs always have a reference, so a missing identity is a
                // check that was skipped.
                if identity.is_none() {
                    return fail("it has no output identity".to_owned());
                }
                // a mechanism that adds sources changed one in every Rust build, cold or edited.
                let adds_sources = mechanism
                    .required_steps(scenario, raw.incremental == Some(true))
                    .is_some_and(|steps| steps.contains(&Step::SourceAdd));
                if adds_sources
                    && matches!(scenario.kind(), Kind::RustBuild(_))
                    && *source_bytes_sent == 0
                {
                    return fail("no source bytes sent for a changed source".to_owned());
                }
                check_runs(raw, *built)
            }
            Self::Substitution {
                substituted,
                expected,
                ..
            } if scenario == Scenario::Substitution => {
                if substituted == expected && *expected > 0 {
                    Ok(())
                } else {
                    fail(format!("{substituted} of {expected} paths substituted"))
                }
            }
            Self::Collection {
                kept,
                expected_kept,
                deleted,
                expected_deleted,
            } if scenario == Scenario::GarbageCollection => {
                if kept == expected_kept && deleted == expected_deleted {
                    Ok(())
                } else {
                    fail(format!(
                        "{kept} of {expected_kept} rooted paths kept and {deleted} of \
                         {expected_deleted} unrooted ones deleted"
                    ))
                }
            }
            Self::Registration {
                registered,
                expected,
            } if scenario == Scenario::PathRegistration => {
                if registered == expected && *expected > 0 {
                    Ok(())
                } else {
                    fail(format!("{registered} of {expected} paths registered"))
                }
            }
            _ => fail("its outcome is of another kind".to_owned()),
        }
    }
}

/// an incremental row's units carried warm state exactly when an earlier build left some: a cold
/// build and a no-op seed nothing, every unit an edit rebuilds starts from its last session, and a
/// seeded unit reused part of it. a seeded row whose sessions rustc ignored, as it does in silence
/// for a session without its lock file, reuses nothing and is refused.
fn check_warm(raw: &RawRecord, built: u32, warm: Option<&Warm>) -> Result<(), ContractError> {
    let scenario = raw.scenario;
    let fail = |detail: String| Err(refuse(scenario, detail));
    let incremental = raw.incremental == Some(true);
    let Some(warm) = warm else {
        return if incremental {
            fail("an incremental row says nothing of its warm state".to_owned())
        } else {
            Ok(())
        };
    };
    if !incremental {
        return fail("a row without incremental units has warm state".to_owned());
    }
    let expected = if scenario.edit().is_some() { built } else { 0 };
    if warm.seeded != expected {
        return fail(format!(
            "{} of {built} built units were seeded, where {expected} had a session to start from",
            warm.seeded
        ));
    }
    if warm.seeded > 0 && warm.reused == 0 {
        return fail(format!(
            "{} units were seeded and rustc reused none of their {} work products",
            warm.seeded, warm.work_products
        ));
    }
    if warm.reused > warm.work_products {
        return fail("more work products reused than written".to_owned());
    }
    Ok(())
}

/// each unit an executor built ran its own step: a derivation per unit is one build request per
/// unit, a batch is one request for one or more, and the owned sandbox compiles each unit once.
fn check_runs(raw: &RawRecord, built: u32) -> Result<(), ContractError> {
    let count = |step: Step| {
        raw.buckets
            .iter()
            .find(|b| b.step == step)
            .map_or(0, |b| b.count)
    };
    let mechanism = raw.candidate.mechanism;
    let (step, fits): (Step, fn(u32, u32) -> bool) = match mechanism {
        Mechanism::DerivationPerUnit => (Step::Build, |runs, built| runs == built),
        Mechanism::BatchedDerivations => (Step::Build, |runs, built| {
            (built == 0) == (runs == 0) && runs <= built
        }),
        Mechanism::OwnedSandbox => (Step::Compile, |runs, built| runs == built),
        Mechanism::Cargo | Mechanism::Store => return Ok(()),
    };
    let runs = count(step);
    if fits(runs, built) {
        Ok(())
    } else {
        Err(refuse(
            raw.scenario,
            format!("{mechanism:?} ran {step:?} {runs} times for {built} units built"),
        ))
    }
}

/// what a mechanism's records say about where they ran: Cargo has no Nix, no transport and a
/// process-tree CPU scope, and incremental is set; an executor has all three of the others, and
/// says whether its units were incremental whenever it compiles.
fn check_context(raw: &RawRecord) -> Result<(), ContractError> {
    let mechanism = raw.candidate.mechanism;
    let fail = |detail: &str| {
        Err(refuse(
            raw.scenario,
            format!("a {mechanism:?} record {detail}"),
        ))
    };
    match mechanism {
        Mechanism::Cargo => {
            if raw.nix.is_some() || raw.transport.is_some() {
                return fail("names a Nix or a transport, which Cargo has none of");
            }
            if raw.incremental.is_none() || raw.cpu.scope != CpuScope::ProcessTree {
                return fail("must say whether incremental was on and count its process tree");
            }
        }
        Mechanism::DerivationPerUnit | Mechanism::BatchedDerivations | Mechanism::OwnedSandbox => {
            if raw.nix.is_none() || raw.transport.is_none() {
                return fail("must name the Nix and the transport it ran against");
            }
            if raw.cpu.scope != CpuScope::Machine {
                return fail("must count the machine's CPU, where the daemon's work shows");
            }
            if raw.incremental.is_some() != raw.scenario.compiles() {
                return fail("says whether its units are incremental exactly when it compiles");
            }
        }
        Mechanism::Store => {
            if raw.nix.is_none() || raw.transport.is_none() {
                return fail("must name the Nix and the transport it ran against");
            }
        }
    }
    if raw.covered_ns > raw.wall_time_ns {
        return fail("covers more time with its buckets than its wall time");
    }
    Ok(())
}

impl Record {
    /// makes a record from what a writer measured, checking it as a reader would.
    pub fn new(parts: RecordParts) -> Result<Self, ContractError> {
        let RecordParts {
            started_at_unix_ms,
            machine,
            nix,
            candidate,
            transport,
            fixture,
            scenario,
            incremental,
            iteration,
            wall_time_ns,
            cpu,
            buckets,
            covered_ns,
            outcome,
            store_valid_paths,
        } = parts;
        let model = fixture.model()?;
        let raw = RawRecord {
            schema_version: RecordVersion::default(),
            started_at_unix_ms,
            machine,
            nix,
            candidate,
            transport,
            fixture,
            scenario,
            incremental,
            unit_count: model.graph.units,
            graph_depth: model.graph.depth,
            iteration,
            wall_time_ns,
            cpu,
            buckets,
            covered_ns,
            outcome,
            store_valid_paths,
        };
        check(&raw, &model)?;
        Ok(Self { raw })
    }

    pub fn fixture(&self) -> &Fixture {
        &self.raw.fixture
    }

    pub fn scenario(&self) -> Scenario {
        self.raw.scenario
    }

    pub fn incremental(&self) -> Option<bool> {
        self.raw.incremental
    }

    pub fn nix(&self) -> Option<&NixImplementation> {
        self.raw.nix.as_ref()
    }

    pub fn outcome(&self) -> &Outcome {
        &self.raw.outcome
    }

    pub fn graph(&self) -> GraphFacts {
        GraphFacts {
            units: self.raw.unit_count,
            depth: self.raw.graph_depth,
        }
    }

    pub fn to_json_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("a record always serializes");
        line.push('\n');
        line
    }

    pub fn from_json_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

/// checks what a record can't check alone: that its fixture regenerates to the digest it names.
/// regenerating a large workspace costs milliseconds, so each fixture is regenerated once, keyed
/// by what its bytes depend on.
#[derive(Debug, Default)]
pub struct Validator {
    digests: HashMap<FixtureKey, Sha256>,
}

impl Validator {
    pub fn check(&mut self, record: &Record) -> Result<(), ContractError> {
        let fixture = record.fixture();
        let regenerated = match self.digests.get(&fixture.key()) {
            Some(digest) => *digest,
            None => {
                let digest = fixture.regenerated_digest()?;
                self.digests.insert(fixture.key(), digest);
                digest
            }
        };
        if regenerated == fixture.digest() {
            Ok(())
        } else {
            Err(ContractError(format!(
                "the fixture's digest {} isn't what its seed and parameters regenerate, {regenerated}",
                fixture.digest()
            )))
        }
    }
}

pub fn record_schema() -> Schema {
    schemars::schema_for!(Record)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn params() -> Value {
        json!({
            "crates": 4,
            "depth": 2,
            "dependencies_per_crate": 2,
            "modules_per_crate": 2,
            "functions_per_module": 3,
            "generic_percent": 25,
            "large_crate": null
        })
    }

    fn workspace() -> RustWorkspace {
        RustWorkspace::generate(7, serde_json::from_value(params()).unwrap()).unwrap()
    }

    fn valid() -> Value {
        let digest = workspace().render().digest().to_string();
        json!({
            "schema_version": 2,
            "started_at_unix_ms": 1_790_000_000_000u64,
            "machine": {
                "name": "bench-host",
                "cpu_model": null,
                "logical_cpus": 16,
                "os": "linux",
                "arch": "x86_64"
            },
            "nix": null,
            "candidate": { "name": "cargo", "version": "cargo 1.99.0", "mechanism": "cargo" },
            "transport": null,
            "fixture": {
                "kind": "rust-workspace",
                "seed": 7,
                "parameters": params(),
                "digest": digest
            },
            "scenario": "one-function-edit",
            "incremental": true,
            "unit_count": 4,
            "graph_depth": 2,
            "iteration": 0,
            "wall_time_ns": 1_500_000_000u64,
            "cpu": { "scope": "process-tree", "user_ns": 2_000_000_000u64, "system_ns": 300_000_000u64 },
            "buckets": [
                { "step": "compile", "count": 1, "total_ns": 1_400_000_000u64, "first_start_ns": 50_000_000u64, "last_end_ns": 1_450_000_000u64 }
            ],
            "covered_ns": 1_400_000_000u64,
            "outcome": {
                "kind": "build",
                "built": 1,
                "skipped": 3,
                "expected_built": 1,
                "source_bytes_sent": 0,
                "output_bytes_sent": 0,
                "identity": null,
                "warm": null
            },
            "store_valid_paths": null
        })
    }

    fn bucket(step: &str) -> Value {
        json!({ "step": step, "count": 12, "total_ns": 30, "first_start_ns": 0, "last_end_ns": 100 })
    }

    fn with_nix() -> Value {
        let mut value = valid();
        value["nix"] = json!({
            "implementation": "determinate",
            "version": "3.24.2",
            "environment": "virtual-machine",
            "settings": {
                "sandbox": { "state": "set", "value": "enabled" },
                "use_cgroups": { "state": "set", "value": false },
                "max_jobs": { "state": "set", "value": 16 },
                "cores": { "state": "set", "value": 0 }
            },
            "client_trust": "untrusted"
        });
        value["candidate"] = json!({
            "name": "derivation-per-unit",
            "version": "shikumi-exec 0.1.0",
            "mechanism": "derivation-per-unit"
        });
        value["transport"] = json!("daemon-socket");
        value["fixture"] = json!({
            "kind": "overhead",
            "shape": "chain",
            "units": 12,
            "digest": "0".repeat(64)
        });
        value["scenario"] = json!("latency-floor");
        value["unit_count"] = json!(12);
        value["graph_depth"] = json!(11);
        value["incremental"] = json!(null);
        value["cpu"]["scope"] = json!("machine");
        value["buckets"] = json!(
            [
                "connect",
                "drv-synthesis",
                "drv-write",
                "build",
                "readdress",
                "rooting"
            ]
            .map(bucket)
        );
        value["outcome"] = json!({
            "kind": "build",
            "built": 12,
            "skipped": 0,
            "expected_built": 12,
            "source_bytes_sent": 0,
            "output_bytes_sent": 1_200,
            "identity": { "outputs": "1".repeat(64), "reference": "1".repeat(64) },
            "warm": null
        });
        value
    }

    fn parse(value: &Value) -> Result<Record, serde_json::Error> {
        Record::from_json_line(&value.to_string())
    }

    fn rejects_from(base: Value, mutate: impl FnOnce(&mut Value), expected: &str) {
        let mut value = base;
        mutate(&mut value);
        let error = parse(&value)
            .expect_err("malformed record was accepted")
            .to_string();
        assert!(
            error.contains(expected),
            "error {error:?} doesn't mention {expected:?}"
        );
    }

    fn rejects(mutate: impl FnOnce(&mut Value), expected: &str) {
        rejects_from(valid(), mutate, expected);
    }

    #[test]
    fn accepts_and_round_trips_valid_records() {
        for value in [valid(), with_nix()] {
            let record = parse(&value).unwrap();
            let line = record.to_json_line();
            assert_eq!(Record::from_json_line(&line).unwrap(), record);
            assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), value);
        }
    }

    // a setting an implementation doesn't have is recorded as such, never as a default value.
    #[test]
    fn a_setting_the_implementation_lacks_is_recorded_as_unsupported() {
        let mut unsupported = with_nix();
        unsupported["nix"]["settings"]["use_cgroups"] = json!({ "state": "unsupported" });
        assert!(parse(&unsupported).is_ok());
    }

    #[test]
    fn rejects_a_missing_required_field() {
        rejects(
            |v| drop(v.as_object_mut().unwrap().remove("wall_time_ns")),
            "wall_time_ns",
        );
        rejects_from(
            with_nix(),
            |v| {
                drop(
                    v["nix"]["settings"]
                        .as_object_mut()
                        .unwrap()
                        .remove("cores"),
                )
            },
            "cores",
        );
    }

    #[test]
    fn rejects_a_missing_nullable_field() {
        // null is the explicit "doesn't apply"; leaving the field out is a malformed record.
        rejects(
            |v| drop(v.as_object_mut().unwrap().remove("transport")),
            "transport",
        );
        rejects(|v| drop(v.as_object_mut().unwrap().remove("nix")), "nix");
        rejects(
            |v| drop(v["outcome"].as_object_mut().unwrap().remove("identity")),
            "identity",
        );
    }

    #[test]
    fn rejects_a_wrong_type() {
        rejects(|v| v["wall_time_ns"] = json!("1.5s"), "invalid type");
        rejects(|v| v["incremental"] = json!("yes"), "invalid type");
    }

    #[test]
    fn rejects_sentinels() {
        rejects(|v| v["transport"] = json!(""), "unknown variant");
        rejects(|v| v["wall_time_ns"] = json!(-1), "invalid value");
        rejects(|v| v["candidate"]["version"] = json!(" "), "empty");
        rejects_from(
            with_nix(),
            |v| v["nix"]["settings"]["sandbox"] = json!({ "state": "unknown" }),
            "unknown variant",
        );
    }

    #[test]
    fn rejects_unknown_fields_and_versions() {
        rejects(|v| v["wall_time_s"] = json!(1.5), "unknown field");
        rejects(|v| v["schema_version"] = json!(1), "schema_version 1");
        rejects(|v| v["scenario"] = json!("warm"), "unknown variant");
        rejects(
            |v| v["buckets"][0]["step"] = json!("lunch"),
            "unknown variant",
        );
    }

    #[test]
    fn rejects_a_graph_that_contradicts_the_fixture() {
        rejects(
            |v| v["unit_count"] = json!(12),
            "doesn't match the fixture's 4 units",
        );
        rejects_from(
            with_nix(),
            |v| v["unit_count"] = json!(4),
            "fixture's 12 units",
        );
        rejects(|v| v["graph_depth"] = json!(3), "depth 2");
        rejects_from(with_nix(), |v| v["graph_depth"] = json!(12), "depth 11");
    }

    #[test]
    fn rejects_a_fixture_that_cannot_be_generated() {
        fn params(v: &mut Value) -> &mut Value {
            &mut v["fixture"]["parameters"]
        }
        rejects(|v| params(v)["crates"] = json!(0), "invalid value");
        rejects(|v| params(v)["depth"] = json!(0), "invalid value");
        rejects(|v| params(v)["depth"] = json!(4), "can't fill");
        rejects(
            |v| {
                params(v)["large_crate"] =
                    json!({ "modules": 2, "functions_per_module": 2, "dependencies": 9 })
            },
            "large crate's dependencies",
        );
        rejects_from(
            with_nix(),
            |v| v["fixture"]["units"] = json!(0),
            "invalid value",
        );
    }

    #[test]
    fn rejects_a_scenario_its_fixture_cannot_run() {
        rejects(|v| v["scenario"] = json!("latency-floor"), "can't run");
        rejects_from(
            with_nix(),
            |v| v["scenario"] = json!("unit-overhead"),
            "can't run",
        );
    }

    // the timer controls: a step the mechanism pays that has no bucket, and a bucket whose span
    // falls outside the build, are both a timer that missed a step.
    #[test]
    fn rejects_a_row_whose_timer_misses_a_step() {
        rejects_from(
            with_nix(),
            |v| drop(v["buckets"].as_array_mut().unwrap().remove(4)),
            "Readdress",
        );
        rejects(
            |v| v["buckets"][0]["last_end_ns"] = json!(1_600_000_000u64),
            "outside the timer",
        );
        rejects(
            |v| {
                let compile = v["buckets"][0].clone();
                v["buckets"].as_array_mut().unwrap().push(compile);
            },
            "appears twice",
        );
        rejects_from(
            with_nix(),
            |v| v["candidate"]["mechanism"] = json!("store"),
            "has no",
        );
    }

    // the outcome controls: a cached result standing in for a rebuild, an edit that rebuilt more
    // or less than the model says, a cutoff that moved output bytes, and a wrong output.
    #[test]
    fn rejects_a_build_that_did_other_work_than_its_row_claims() {
        rejects(
            |v| {
                v["outcome"]["built"] = json!(0);
                v["outcome"]["skipped"] = json!(4);
            },
            "0 units built, 1 expected",
        );
        rejects(
            |v| v["outcome"]["expected_built"] = json!(4),
            "model gives 1",
        );
        rejects(|v| v["outcome"]["skipped"] = json!(2), "don't make");
        rejects_from(
            with_nix(),
            |v| v["outcome"]["identity"]["outputs"] = json!("2".repeat(64)),
            "differs from the reference",
        );
        let mut cutoff = with_nix();
        cutoff["candidate"]["mechanism"] = json!("owned-sandbox");
        cutoff["fixture"] = valid()["fixture"].clone();
        cutoff["unit_count"] = json!(4);
        cutoff["graph_depth"] = json!(2);
        cutoff["scenario"] = json!("identical-output-edit");
        cutoff["buckets"] = json!(
            [
                "state",
                "connect",
                "source-scan",
                "validity-query",
                "process-start",
                "sandbox-setup",
                "compile",
                "output-add",
                "rooting"
            ]
            .map(bucket)
        );
        cutoff["incremental"] = json!(false);
        cutoff["buckets"][6]["count"] = json!(1);
        cutoff["outcome"]["built"] = json!(1);
        cutoff["outcome"]["skipped"] = json!(3);
        cutoff["outcome"]["expected_built"] = json!(1);
        cutoff["outcome"]["output_bytes_sent"] = json!(0);
        parse(&cutoff).unwrap();
        rejects_from(
            cutoff,
            |v| v["outcome"]["output_bytes_sent"] = json!(512),
            "nothing new can be produced",
        );
        rejects(
            |v| {
                v["scenario"] = json!("no-op");
                v["outcome"]["built"] = json!(0);
                v["outcome"]["skipped"] = json!(4);
                v["outcome"]["expected_built"] = json!(0);
                v["outcome"]["source_bytes_sent"] = json!(10);
            },
            "with nothing changed",
        );
        rejects(
            |v| v["outcome"] = json!({ "kind": "registration", "registered": 1, "expected": 1 }),
            "another kind",
        );
        rejects(
            |v| v["outcome"]["warm"] = json!({ "seeded": 0, "work_products": 1, "reused": 0 }),
            "not warm state",
        );
    }

    // the seeded controls: a seeded row whose units didn't carry their sessions in, or whose
    // sessions rustc ignored, isn't the configuration it's named for.
    #[test]
    fn rejects_a_seeded_row_that_did_not_seed() {
        let mut seeded = with_nix();
        seeded["candidate"]["mechanism"] = json!("derivation-per-unit");
        seeded["fixture"] = valid()["fixture"].clone();
        seeded["unit_count"] = json!(4);
        seeded["graph_depth"] = json!(2);
        seeded["scenario"] = json!("one-function-edit");
        seeded["incremental"] = json!(true);
        seeded["buckets"] = json!(
            [
                "state",
                "connect",
                "source-scan",
                "source-add",
                "validity-query",
                "drv-synthesis",
                "drv-write",
                "build",
                "readdress",
                "rooting"
            ]
            .map(bucket)
        );
        seeded["buckets"][7]["count"] = json!(1);
        seeded["outcome"]["built"] = json!(1);
        seeded["outcome"]["skipped"] = json!(3);
        seeded["outcome"]["expected_built"] = json!(1);
        seeded["outcome"]["warm"] = json!({ "seeded": 1, "work_products": 40, "reused": 38 });
        seeded["outcome"]["source_bytes_sent"] = json!(100);
        parse(&seeded).unwrap();
        rejects_from(
            seeded.clone(),
            |v| v["outcome"]["source_bytes_sent"] = json!(0),
            "no source bytes sent",
        );
        rejects_from(
            seeded.clone(),
            |v| v["outcome"]["warm"]["seeded"] = json!(0),
            "had a session to start from",
        );
        rejects_from(
            seeded.clone(),
            |v| v["outcome"]["warm"]["reused"] = json!(0),
            "reused none",
        );
        rejects_from(
            seeded.clone(),
            |v| v["outcome"]["warm"] = json!(null),
            "says nothing of its warm state",
        );
        rejects_from(
            seeded.clone(),
            |v| v["incremental"] = json!(false),
            "has warm state",
        );
        rejects_from(
            seeded,
            |v| {
                v["scenario"] = json!("cold");
                v["outcome"]["built"] = json!(4);
                v["outcome"]["skipped"] = json!(0);
                v["outcome"]["expected_built"] = json!(4);
                v["outcome"]["source_bytes_sent"] = json!(100);
                v["buckets"][7]["count"] = json!(4);
            },
            "where 0 had a session",
        );
    }

    // a row that could pass while not being what it's named for: a warm store standing in for a
    // cold one, a skipped identity, a run count that can't match the units built, or a context
    // the mechanism doesn't run in.
    #[test]
    fn rejects_an_executor_row_that_cannot_be_what_it_claims() {
        let latency = with_nix();
        rejects_from(
            latency.clone(),
            |v| v["outcome"]["identity"] = json!(null),
            "no output identity",
        );
        rejects_from(
            latency.clone(),
            |v| v["outcome"]["output_bytes_sent"] = json!(0),
            "every built output is new",
        );
        rejects_from(
            latency.clone(),
            |v| v["buckets"][3]["count"] = json!(1),
            "ran Build 1 times for 12 units built",
        );
        rejects_from(
            latency.clone(),
            |v| v["transport"] = json!(null),
            "transport",
        );
        rejects_from(latency.clone(), |v| v["nix"] = json!(null), "transport");
        rejects_from(
            latency.clone(),
            |v| v["cpu"]["scope"] = json!("process-tree"),
            "machine's CPU",
        );
        rejects_from(
            latency.clone(),
            |v| v["incremental"] = json!(true),
            "incremental",
        );
        rejects_from(
            latency.clone(),
            |v| v["covered_ns"] = json!(2_000_000_000u64),
            "more time",
        );
        let mut batched = latency;
        batched["candidate"]["mechanism"] = json!("batched-derivations");
        batched["buckets"][3]["count"] = json!(2);
        assert!(parse(&batched).is_ok());
        rejects_from(
            batched,
            |v| v["buckets"][3]["count"] = json!(13),
            "ran Build 13 times",
        );
        rejects(|v| v["incremental"] = json!(null), "incremental");
        rejects(|v| v["transport"] = json!("cli"), "Cargo has none of");
    }

    #[test]
    fn a_fixture_whose_digest_does_not_regenerate_is_refused() {
        let mut validator = Validator::default();
        let good = parse(&valid()).unwrap();
        validator.check(&good).unwrap();
        let mut stale = valid();
        stale["fixture"]["digest"] = json!("0".repeat(64));
        let error = validator
            .check(&parse(&stale).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("isn't what its seed"), "{error}");
        let mut overhead = with_nix();
        overhead["fixture"]["digest"] = json!(
            crate::fixture::overhead::UnitGraph::generate(
                Shape::Chain,
                NonZeroU32::new(12).unwrap()
            )
            .digest()
            .to_string()
        );
        validator.check(&parse(&overhead).unwrap()).unwrap();
        assert!(validator.check(&parse(&with_nix()).unwrap()).is_err());
    }

    #[test]
    fn schema_requires_every_field_including_nullable_ones() {
        let schema = serde_json::to_value(record_schema()).unwrap();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let properties = schema["properties"].as_object().unwrap();
        assert_eq!(required.len(), properties.len());
        for name in ["nix", "transport", "incremental", "store_valid_paths"] {
            assert!(required.contains(&name), "{name} is not required");
            let types = &properties[name];
            assert!(
                types.to_string().contains("null"),
                "{name} doesn't admit null: {types}"
            );
        }
        let crates = &schema["$defs"]["RustWorkspaceParams"]["properties"]["crates"];
        assert_eq!(crates["minimum"], json!(1), "{crates}");
    }
}
