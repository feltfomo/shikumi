//! producing the registry document: evaluating the declaration layer through the host's
//! `nix-instantiate`, reading the declarations it serializes, checking them, and handing the
//! document to the shared refusal.
//!
//! each project's declaration is evaluated on its own terms, so a failure in one comes back as that
//! project's finding and every other project is still checked in the same run. one evaluation plans
//! the composition: it reads the manifest and names each listed project's declaration file, and
//! reads no declaration. the projects are then evaluated in batches, as many at once as the process
//! has cores, and each batch's answer is read on Rayon's pool as it arrives. a batch that fails
//! names the project it failed in, by the frame `produce.nix` puts around each one; that project
//! gets the failure, and the projects before and after it are evaluated again without it. the
//! evaluator is pure and a failed evaluation leaves nothing behind, so a project comes out exactly
//! as it would evaluated alone. a focused composition is evaluated the same way, in rounds: the
//! focused project, then the listed projects its serialized `dependsOn` and toolchain imports name,
//! and so on, so its projects come out as the global composition gives them.
//!
//! the catalog is the one evaluation that reads a package set. it's read once for each system the
//! manifest names, by an evaluation of its own that goes to the front of the first round's queue,
//! since a package set takes longest to evaluate, and runs beside the projects' batches. no
//! declaration reads a package set, so the batches cost what they did without one, and a catalog
//! that fails to evaluate is the catalog's finding while every project is still evaluated.
//!
//! cost grows linearly with the projects, since a batch reads its declaration files without the
//! manifest and holds at most `BATCH_PROJECTS`. a failure a frame places in its project costs at
//! most one batch and two evaluator starts of about twenty milliseconds: a project that evaluated is
//! evaluated at most twice, once in the batch that failed after it, once in the batch of those
//! before the failure. a failure no frame places, such as a crash, halves its batch down to its
//! project, so it costs about two batches and thirteen starts, and a project beside it is evaluated
//! up to seven times. a focused composition takes one round per level of its dependencies. the
//! catalog adds one evaluation per system, whatever the number of projects or batches.
//!
//! every evaluation is built by `command`, so none of them can reach a build or read a build's
//! output through the evaluator. each runs against `dummy://`, a store that holds nothing and
//! accepts nothing, so every derivation output it would import, read, hash or test for is missing on
//! every host, whatever the host's store holds. import-from-derivation is off as well, which is the
//! same refusal stated as the setting the three implementations share. the settings that would hand
//! a declaration another way to run or build something, `builtins.exec`, plugins and experimental
//! builtins, are pinned off on the command line, which outranks every configuration file and
//! `NIX_CONFIG`. so are the settings that shape the error a failure is read from.
//!
//! the child gets no environment at all, so `builtins.getEnv` reads nothing and a caller's `PWD`,
//! `TMPDIR` or `NIX_PATH` can't make the document depend on where it was produced. a path read
//! straight from the filesystem, with no store context, is out of reach of all of this.
//!
//! the evaluator processes belong to the run that started them. each is started and waited for by
//! one of the run's scoped threads, which end before the run returns, so every child is reaped
//! before `declarations` returns, on success and on failure alike. after an error no new batch
//! starts and the batches already running finish. Ctrl-C reaches the evaluators directly, since they
//! share the terminal's process group. a `shikumi` killed any other way leaves its running
//! evaluators to finish their batch, at most `BATCH_PROJECTS` declarations, and exit, or, for a
//! declaration that never terminates, to run until they're killed too. tying each
//! child to its parent's death would take a hook run between fork and exec, which makes the
//! standard library fork the whole process rather than spawn the child: measured at 10000
//! projects, that cost 2.4 s of CPU and 0.6 s of wall time per run.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Condvar, Mutex, PoisonError};

use crate::check;
use crate::contract::RelativePath;
use crate::declarations::{
    Catalog, CatalogRead, Declaration, Declarations, Evaluated, Failed, Failure, FailureKind, Node,
};
use crate::document::{Document, Identifier, Registry, RegistryError};
use crate::evaluation::{Answer, Evaluation, PlannedCatalog, Project, Request};
use crate::failure::{Classified, Places, classify};
use crate::layer::MANIFEST;
use crate::vocabulary::names;

/// how many batches each core gets, so a batch of slow declarations doesn't leave the other cores
/// idle at the end of the run.
const BATCHES_PER_CORE: usize = 4;

/// the most projects one batch holds. a failure evaluates again the projects its batch evaluated
/// before it, so this bounds what each failure costs whatever the repository's size; a batch this
/// size costs a few times what starting its evaluator does.
const BATCH_PROJECTS: usize = 64;

/// the most bytes of project names one batch's request carries. a command-line argument can't be
/// longer than 128 KiB on Linux.
const REQUEST_BYTES: usize = 64 * 1024;

/// where the producer finds what it evaluates with: the host's `nix-instantiate`, Shikumi's own
/// declaration layer and the nixpkgs source tree whose `lib` the layer runs on.
#[derive(Debug, Clone)]
pub struct Producer {
    nix_instantiate: PathBuf,
    layer: PathBuf,
    nixpkgs: PathBuf,
}

impl Producer {
    pub fn new(nix_instantiate: PathBuf, layer: PathBuf, nixpkgs: PathBuf) -> Self {
        Self {
            nix_instantiate,
            layer,
            nixpkgs,
        }
    }

    /// the producer an installed `shikumi` runs with. its package sets `SHIKUMI_NIX` to the
    /// declaration layer it ships and `SHIKUMI_NIXPKGS` to the nixpkgs it was built against, and
    /// `nix-instantiate` comes from the caller's `PATH`, since the host's evaluator is the one
    /// that reads a repository's Nix.
    pub fn from_environment() -> Result<Self, ProduceError> {
        let required = |name: &'static str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .ok_or(ProduceError::Unset(name))
        };
        let layer = required("SHIKUMI_NIX")?;
        let nixpkgs = required("SHIKUMI_NIXPKGS")?;
        let path = std::env::var_os("PATH").unwrap_or_default();
        let nix_instantiate =
            find_program("nix-instantiate", &path).ok_or(ProduceError::NoEvaluator)?;
        Ok(Self::new(nix_instantiate, layer, nixpkgs))
    }

    /// composes the repository at `root`, globally or focused on one project, and refuses the
    /// document unless it's accepted.
    pub fn produce(
        &self,
        root: &Path,
        focus: Option<&Identifier>,
    ) -> Result<Registry, ProduceError> {
        Registry::try_from(self.document(root, focus)?).map_err(ProduceError::Document)
    }

    /// the document the checker built, findings included, before the refusal.
    pub fn document(
        &self,
        root: &Path,
        focus: Option<&Identifier>,
    ) -> Result<Document, ProduceError> {
        let (declarations, root) = self.declarations(root, focus)?;
        check::check(&declarations, &utf8(&root)?).map_err(ProduceError::Check)
    }

    /// the declarations as the declaration layer serialized them, each failure in its place, and
    /// the canonical root they were evaluated from.
    pub fn declarations(
        &self,
        root: &Path,
        focus: Option<&Identifier>,
    ) -> Result<(Declarations, PathBuf), ProduceError> {
        let root = std::fs::canonicalize(root).map_err(|error| ProduceError::Root {
            root: root.to_owned(),
            error,
        })?;
        if !root.join(MANIFEST.name).is_file() {
            return Err(ProduceError::NoManifest(root));
        }
        let root_text = utf8(&root)?;
        // as the evaluator prints them, which is with every symlink resolved.
        let canonical =
            |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let evaluator = Evaluator {
            producer: self,
            root: &root,
            places: Places {
                root: &root_text,
                layer: &utf8(&canonical(&self.layer))?,
                nixpkgs: &utf8(&canonical(&self.nixpkgs))?,
            },
        };
        let parallelism = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        let declarations = compose(&evaluator, focus.map(Identifier::as_str), parallelism)?;
        Ok((declarations, root))
    }

    // TODO: evaluate in pure mode from the context cache's store copy of exactly the keyed files,
    // once the context cache exists. until then a declaration can read a store path whose context
    // it stripped, an untracked file, `.git` or the clock, and the document can depend on them.
    fn command(&self, root: &Path, request: &Request<'_>) -> Result<Command, ProduceError> {
        let mut command = Command::new(&self.nix_instantiate);
        command
            .args(["--eval", "--strict", "--json"])
            .args(["--store", "dummy://"])
            .args(["--option", "allow-import-from-derivation", "false"])
            .args(["--option", "nix-path", ""])
            .args([
                "--option",
                "allow-unsafe-native-code-during-evaluation",
                "false",
            ])
            .args(["--option", "plugin-files", ""])
            .args(["--option", "experimental-features", ""])
            // every frame, in the plain format the failure is read from, whatever the host's
            // configuration asks for.
            .args(["--show-trace", "--log-format", "raw"])
            .arg(self.layer.join("produce.nix"))
            .args(["--argstr", "nixpkgs"])
            .arg(utf8(&self.nixpkgs)?)
            .args(["--argstr", "root"])
            .arg(utf8(root)?)
            .args(["--argstr", "request"])
            .arg(request.to_json());
        command.env_clear().current_dir("/");
        Ok(command)
    }
}

/// what one evaluation printed: its answer, or the error it failed with.
#[derive(Debug)]
enum Printed {
    Answer(Vec<u8>),
    Error(String),
}

/// one evaluation of the declaration layer. the producer's evaluator runs `nix-instantiate`; the
/// tests' stands in for it.
trait Evaluate: Sync {
    fn evaluate(&self, request: &Request<'_>) -> Result<Printed, ProduceError>;
    fn places(&self) -> Places<'_>;
}

struct Evaluator<'p> {
    producer: &'p Producer,
    root: &'p Path,
    places: Places<'p>,
}

impl Evaluate for Evaluator<'_> {
    fn evaluate(&self, request: &Request<'_>) -> Result<Printed, ProduceError> {
        let output = self
            .producer
            .command(self.root, request)?
            .output()
            .map_err(|error| ProduceError::Spawn {
                program: self.producer.nix_instantiate.clone(),
                error,
            })?;
        Ok(if output.status.success() {
            Printed::Answer(output.stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // a signal leaves nothing on stderr to say so; the status does.
            Printed::Error(if stderr.trim().is_empty() {
                format!("the evaluator exited with {}", output.status)
            } else {
                stderr.into_owned()
            })
        })
    }

    fn places(&self) -> Places<'_> {
        self.places
    }
}

/// an evaluation's answer, held to the shape its request asks for.
fn answer(request: &Request<'_>, printed: &[u8]) -> Result<Answer, ProduceError> {
    let answer = Evaluation::from_json(printed)
        .map(|evaluation| evaluation.answer)
        .map_err(ProduceError::Answer)?;
    if request.answered_by(&answer) {
        Ok(answer)
    } else {
        Err(ProduceError::Unanswered(request.name()))
    }
}

/// plans the composition, evaluates every selected project, and assembles the declarations.
fn compose(
    evaluator: &impl Evaluate,
    focus: Option<&str>,
    parallelism: usize,
) -> Result<Declarations, ProduceError> {
    let plan = match evaluator.evaluate(&Request::Plan {})? {
        Printed::Answer(printed) => match answer(&Request::Plan {}, &printed)? {
            Answer::Plan(plan) => plan,
            Answer::Projects(_) | Answer::Catalog(_) => {
                unreachable!("an answer is held to its request's shape")
            }
        },
        // the plan reads no declaration, so its failure is the manifest's, and with no manifest
        // there's nothing to compose.
        Printed::Error(stderr) => {
            let Classified { failure, .. } = classify(&stderr, evaluator.places());
            return Ok(Declarations::new(
                focus.map(str::to_owned),
                Evaluated::Failed(Failed { failure }),
                None,
                BTreeMap::new(),
            ));
        }
    };

    // the catalog is read once per system, beside the first round's projects; a focused
    // composition reads it whole too, since its shell's packages come from it.
    let mut catalog_jobs: Vec<Job> = match &plan.catalog {
        Some(PlannedCatalog {
            file: Some(file),
            systems,
        }) => systems
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<&str>>()
            .into_iter()
            .map(|system| Job::Catalog {
                file: file.clone(),
                system: system.to_owned(),
            })
            .collect(),
        _ => Vec::new(),
    };

    let mut projects: BTreeMap<String, Entry> = BTreeMap::new();
    let mut catalogs = BTreeMap::new();
    let mut next: BTreeSet<&str> = match focus {
        None => plan.files.keys().map(String::as_str).collect(),
        Some(focus) => plan
            .files
            .get_key_value(focus)
            .map(|(name, _)| name.as_str())
            .into_iter()
            .collect(),
    };
    while !next.is_empty() || !catalog_jobs.is_empty() {
        let mut pending = Vec::new();
        for name in next {
            match &plan.files[name] {
                Some(file) => pending.push(Project {
                    name: name.to_owned(),
                    file: file.clone(),
                }),
                None => {
                    projects.insert(name.to_owned(), None);
                }
            }
        }
        // the catalog's evaluations go first: each reads a whole package set, the slowest
        // evaluations of the run.
        let mut jobs = std::mem::take(&mut catalog_jobs);
        jobs.extend(batches(pending, parallelism).into_iter().map(Job::Projects));
        let evaluated = evaluate(evaluator, jobs, parallelism)?;
        catalogs.extend(evaluated.catalogs);
        // only the projects this round evaluated can name a project not yet composed.
        next = BTreeSet::new();
        if focus.is_some() {
            for (_, entry) in &evaluated.projects {
                next.extend(
                    dependencies(entry)
                        .filter_map(|name| plan.files.get_key_value(name))
                        .map(|(name, _)| name.as_str()),
                );
            }
        }
        projects.extend(evaluated.projects);
        next.retain(|name| !projects.contains_key(*name));
    }
    let catalog = plan.catalog.map(|planned| Catalog {
        file: planned.file,
        systems: catalogs,
    });
    Ok(Declarations::new(
        focus.map(str::to_owned),
        Evaluated::Value(plan.manifest),
        catalog,
        projects,
    ))
}

/// the projects a declaration names in `project.dependsOn` and as the owners in
/// `project.imports`, as it serialized them: each string of the list, and each string value of the
/// attribute set. a focused composition takes them with it, so it holds the projects its project
/// depends on and the toolchains it names. a declaration that failed, or one in any other shape,
/// names none, and the checker reports the shape.
fn dependencies(entry: &Entry) -> impl Iterator<Item = &str> {
    fn field<'n>(node: &'n Node, name: &str) -> Option<&'n Node> {
        match node {
            Node::Attrs { fields } => fields.get(name).map(|field| &field.value),
            _ => None,
        }
    }
    let project = match entry {
        Some(Evaluated::Value(declaration)) if declaration.factory => {
            field(&declaration.value, names::PROJECT)
        }
        _ => None,
    };
    let depends = project
        .and_then(|project| field(project, names::DEPENDS_ON))
        .and_then(|depends| match depends {
            Node::List { items } => Some(items.as_slice()),
            _ => None,
        })
        .unwrap_or_default();
    let imports = project
        .and_then(|project| field(project, names::IMPORTS))
        .and_then(|imports| match imports {
            Node::Attrs { fields } => Some(fields.values().map(|field| &field.value)),
            _ => None,
        })
        .into_iter()
        .flatten();
    depends.iter().chain(imports).filter_map(|item| match item {
        Node::String { value, .. } => Some(value.as_str()),
        _ => None,
    })
}

/// the projects in contiguous batches: at least `BATCHES_PER_CORE` for each core, none of more than
/// `BATCH_PROJECTS`, and none whose request is longer than `REQUEST_BYTES`.
fn batches(projects: Vec<Project>, parallelism: usize) -> Vec<Vec<Project>> {
    let size = projects
        .len()
        .div_ceil(parallelism.max(1) * BATCHES_PER_CORE)
        .clamp(1, BATCH_PROJECTS);
    let mut batches = Vec::new();
    let mut batch: Vec<Project> = Vec::new();
    let mut bytes = 0;
    for project in projects {
        // the project as JSON: its keys and quotes, and room for every character to be escaped.
        let length = (project.name.len() + project.file.as_str().len()) * 2 + 24;
        if !batch.is_empty() && (batch.len() == size || bytes + length > REQUEST_BYTES) {
            batches.push(std::mem::take(&mut batch));
            bytes = 0;
        }
        bytes += length;
        batch.push(project);
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}

/// what one project came to.
type Entry = Option<Evaluated<Declaration>>;

/// what the catalog came to on one system: null for a system the pinned nixpkgs doesn't know.
type Read = Option<Evaluated<CatalogRead>>;

/// one evaluation to run: a batch of projects, or the catalog for one system.
#[derive(Debug)]
enum Job {
    Projects(Vec<Project>),
    Catalog { file: RelativePath, system: String },
}

/// what a round of evaluations came to.
#[derive(Debug, Default)]
struct Results {
    projects: Vec<(String, Entry)>,
    catalogs: Vec<(String, Read)>,
}

/// the work the evaluating threads share: the jobs still to run, how many are running, what came
/// back, and the first error, after which no job starts.
struct Shared {
    queue: VecDeque<Job>,
    running: usize,
    results: Results,
    error: Option<ProduceError>,
}

/// runs every job, at most `parallelism` at a time, and returns what each came to.
fn evaluate(
    evaluator: &impl Evaluate,
    jobs: Vec<Job>,
    parallelism: usize,
) -> Result<Results, ProduceError> {
    let workers = parallelism.min(jobs.len());
    let shared = Mutex::new(Shared {
        queue: jobs.into(),
        running: 0,
        results: Results::default(),
        error: None,
    });
    let changed = Condvar::new();
    // answers are read on Rayon's pool while the threads go on to the next job; the scope waits
    // for every read before it returns.
    rayon::in_place_scope(|reads| {
        std::thread::scope(|threads| {
            for _ in 0..workers {
                threads.spawn(|| work(evaluator, &shared, &changed, reads));
            }
        });
    });
    let shared = recovered(shared.into_inner());
    match shared.error {
        Some(error) => Err(error),
        None => Ok(shared.results),
    }
}

// the shared state stays sound when a thread panics holding it: every change to it is one
// statement, and a panic is reported when the scope rejoins the thread anyway.
fn recovered<T>(result: Result<T, PoisonError<T>>) -> T {
    result.unwrap_or_else(PoisonError::into_inner)
}

fn work<'s>(
    evaluator: &'s impl Evaluate,
    shared: &'s Mutex<Shared>,
    changed: &'s Condvar,
    reads: &rayon::Scope<'s>,
) {
    let lock = || recovered(shared.lock());
    loop {
        let job = {
            let mut state = lock();
            loop {
                if state.error.is_some() {
                    return;
                }
                if let Some(job) = state.queue.pop_front() {
                    state.running += 1;
                    break job;
                }
                // an evaluation still running may yet hand back the projects around a failure.
                if state.running == 0 {
                    return;
                }
                state = recovered(changed.wait(state));
            }
        };
        let outcome = run(evaluator, job);
        let mut state = lock();
        state.running -= 1;
        match outcome {
            Ok(Ran::Answered(names, printed)) => reads.spawn(move |_| {
                let read = read_batch(names, &printed);
                let mut state = lock();
                match read {
                    Ok(entries) => state.results.projects.extend(entries),
                    Err(error) => {
                        state.error.get_or_insert(error);
                    }
                }
                drop(state);
                changed.notify_all();
            }),
            Ok(Ran::Cataloged(file, system, printed)) => reads.spawn(move |_| {
                let read = read_catalog(&file, &system, &printed);
                let mut state = lock();
                match read {
                    Ok(read) => state
                        .results
                        .catalogs
                        .push((system, read.map(Evaluated::Value))),
                    Err(error) => {
                        state.error.get_or_insert(error);
                    }
                }
                drop(state);
                changed.notify_all();
            }),
            Ok(Ran::CatalogFailed(system, failure)) => state
                .results
                .catalogs
                .push((system, Some(Evaluated::Failed(Failed { failure })))),
            Ok(Ran::Failed {
                project,
                failure,
                again,
            }) => {
                state
                    .results
                    .projects
                    .push((project, Some(Evaluated::Failed(Failed { failure }))));
                state.queue.extend(again.into_iter().map(Job::Projects));
            }
            Ok(Ran::Split(halves)) => state.queue.extend(halves.into_iter().map(Job::Projects)),
            Err(error) => {
                state.error.get_or_insert(error);
            }
        }
        drop(state);
        changed.notify_all();
    }
}

/// what running one job came to: a batch's answer, still to be read, or the project it failed in
/// and the batches left to evaluate without it; or the catalog's answer for a system, or its
/// failure.
enum Ran {
    Answered(Vec<Project>, Vec<u8>),
    Cataloged(RelativePath, String, Vec<u8>),
    /// the catalog failed to evaluate. the evaluation reads nothing but the catalog, so the failure
    /// is the catalog's wherever its frames lie.
    CatalogFailed(String, Failure),
    Failed {
        project: String,
        failure: Failure,
        again: Vec<Vec<Project>>,
    },
    /// a failure no frame placed: the batch, halved, to evaluate again.
    Split(Vec<Vec<Project>>),
}

fn run(evaluator: &impl Evaluate, job: Job) -> Result<Ran, ProduceError> {
    match job {
        Job::Projects(batch) => run_batch(evaluator, batch),
        Job::Catalog { file, system } => {
            let request = Request::Catalog {
                file: &file,
                system: &system,
            };
            Ok(match evaluator.evaluate(&request)? {
                Printed::Answer(printed) => Ran::Cataloged(file, system, printed),
                Printed::Error(stderr) => {
                    Ran::CatalogFailed(system, classify(&stderr, evaluator.places()).failure)
                }
            })
        }
    }
}

fn run_batch(evaluator: &impl Evaluate, batch: Vec<Project>) -> Result<Ran, ProduceError> {
    let stderr = match evaluator.evaluate(&Request::Evaluate(&batch))? {
        Printed::Answer(printed) => return Ok(Ran::Answered(batch, printed)),
        Printed::Error(stderr) => stderr,
    };
    let Classified { project, failure } = classify(&stderr, evaluator.places());
    let at = project
        .as_ref()
        .and_then(|name| batch.iter().position(|project| project.name == *name));
    let Some(at) = at else {
        // a failure no frame places in one project: halve the batch until it's down to the one
        // project it fails with.
        if batch.len() > 1 {
            let mut first = batch;
            let second = first.split_off(first.len() / 2);
            return Ok(Ran::Split(vec![first, second]));
        }
        let project = batch.into_iter().next().expect("a batch is never empty");
        return Ok(Ran::Failed {
            project: project.name,
            failure,
            again: Vec::new(),
        });
    };
    let mut before = batch;
    let mut after = before.split_off(at);
    let project = after.remove(0);
    let failure = located(evaluator, &project, failure)?;
    Ok(Ran::Failed {
        project: project.name,
        failure,
        again: [before, after]
            .into_iter()
            .filter(|batch| !batch.is_empty())
            .collect(),
    })
}

/// a failure on text that isn't UTF-8 names no field; evaluating the project once more with every
/// string written out under a frame that names its field says which one it is.
fn located(
    evaluator: &impl Evaluate,
    project: &Project,
    failure: Failure,
) -> Result<Failure, ProduceError> {
    if failure.kind != FailureKind::InvalidUtf8 || failure.field().is_some() {
        return Ok(failure);
    }
    Ok(match evaluator.evaluate(&Request::Locate(project))? {
        Printed::Error(stderr) => {
            let read = classify(&stderr, evaluator.places());
            if read.project.as_deref() == Some(project.name.as_str())
                && read.failure.field().is_some()
            {
                read.failure
            } else {
                failure
            }
        }
        Printed::Answer(_) => failure,
    })
}

fn read_batch(
    projects: Vec<Project>,
    printed: &[u8],
) -> Result<Vec<(String, Entry)>, ProduceError> {
    let declarations = match answer(&Request::Evaluate(&projects), printed)? {
        Answer::Projects(declarations) => declarations,
        Answer::Plan(_) | Answer::Catalog(_) => {
            unreachable!("an answer is held to its request's shape")
        }
    };
    Ok(projects
        .into_iter()
        .zip(declarations)
        .map(|(project, declaration)| (project.name, Some(Evaluated::Value(declaration))))
        .collect())
}

fn read_catalog(
    file: &RelativePath,
    system: &str,
    printed: &[u8],
) -> Result<Option<CatalogRead>, ProduceError> {
    match answer(&Request::Catalog { file, system }, printed)? {
        Answer::Catalog(read) => Ok(read),
        Answer::Plan(_) | Answer::Projects(_) => {
            unreachable!("an answer is held to its request's shape")
        }
    }
}

fn utf8(path: &Path) -> Result<String, ProduceError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| ProduceError::NotUtf8(path.to_owned()))
}

fn find_program(name: &str, path: &OsString) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[derive(Debug)]
pub enum ProduceError {
    Unset(&'static str),
    NoEvaluator,
    Root {
        root: PathBuf,
        error: io::Error,
    },
    NoManifest(PathBuf),
    NotUtf8(PathBuf),
    Spawn {
        program: PathBuf,
        error: io::Error,
    },
    /// the layer printed something that isn't an evaluation answer: a Shikumi bug, since the layer
    /// writes nothing else.
    Answer(serde_json::Error),
    /// the layer answered a request with something other than what it asked for: a Shikumi bug.
    /// the layer answered the named request with something else.
    Unanswered(String),
    Check(check::CheckError),
    Document(RegistryError),
}

impl fmt::Display for ProduceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unset(name) => write!(
                f,
                "{name} is unset; it's set by the shikumi package, so run shikumi from it"
            ),
            Self::NoEvaluator => {
                f.write_str("no nix-instantiate on PATH to evaluate declarations with")
            }
            Self::Root { root, error } => write!(f, "opening {}: {error}", root.display()),
            Self::NoManifest(root) => {
                write!(f, "{} has no {} manifest", root.display(), MANIFEST.name)
            }
            Self::NotUtf8(path) => write!(f, "{} isn't valid UTF-8", path.display()),
            Self::Spawn { program, error } => write!(f, "running {}: {error}", program.display()),
            Self::Answer(error) => write!(
                f,
                "the declaration layer's output isn't an evaluation answer, which is a Shikumi bug: {error}"
            ),
            Self::Unanswered(request) => write!(
                f,
                "the declaration layer answered a {request} request with something else, which is a Shikumi bug"
            ),
            Self::Check(error) => error.fmt(f),
            Self::Document(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ProduceError {}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::contract::RelativePath;

    fn producer() -> Producer {
        Producer::new(
            PathBuf::from("/bin/nix-instantiate"),
            PathBuf::from("/layer"),
            PathBuf::from("/nixpkgs"),
        )
    }

    fn project(name: &str) -> Project {
        Project {
            name: name.to_owned(),
            file: RelativePath::new(format!("{name}/project.nix")).unwrap(),
        }
    }

    fn projects(names: &[String]) -> Vec<Project> {
        names.iter().map(|name| project(name)).collect()
    }

    fn user_frames(failure: &Failure) -> &[crate::declarations::At] {
        match &failure.origin {
            crate::declarations::Origin::User(origin) => &origin.frames,
            crate::declarations::Origin::Shikumi(_) => panic!("{failure:?}"),
        }
    }

    fn plan() -> Request<'static> {
        Request::Plan {}
    }

    #[test]
    fn evaluation_can_reach_no_store_and_nothing_that_builds_or_runs() {
        let command = producer().command(Path::new("/repo"), &plan()).unwrap();
        let args: Vec<&OsStr> = command.get_args().collect();
        let has = |pair: [&str; 2]| args.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]);
        assert!(has(["--store", "dummy://"]), "{args:?}");
        for (setting, value) in [
            ("allow-import-from-derivation", "false"),
            ("allow-unsafe-native-code-during-evaluation", "false"),
            ("plugin-files", ""),
            ("experimental-features", ""),
        ] {
            let pinned = args.windows(3).any(|w| w == ["--option", setting, value]);
            assert!(pinned, "{setting} isn't pinned to {value:?}: {args:?}");
        }
        assert!(has(["--log-format", "raw"]), "{args:?}");
        assert!(args.contains(&OsStr::new("--show-trace")), "{args:?}");
    }

    #[test]
    fn every_request_runs_with_the_same_refusals() {
        let names = [project("a")];
        let pinned = |request: &Request<'_>| {
            let command = producer().command(Path::new("/repo"), request).unwrap();
            let args: Vec<String> = command
                .get_args()
                .map(|a| a.to_str().unwrap().to_owned())
                .collect();
            let request_at = args.iter().position(|a| a == "request").unwrap();
            args[..request_at].to_vec()
        };
        let planned = pinned(&plan());
        let catalog = RelativePath::new("catalog/catalog.nix").unwrap();
        assert_eq!(pinned(&Request::Evaluate(&names)), planned);
        assert_eq!(pinned(&Request::Locate(&names[0])), planned);
        assert_eq!(
            pinned(&Request::Catalog {
                file: &catalog,
                system: "x86_64-linux"
            }),
            planned
        );
    }

    #[test]
    fn the_child_gets_no_environment() {
        let command = producer().command(Path::new("/repo"), &plan()).unwrap();
        let passed: Vec<_> = command.get_envs().collect();
        assert!(passed.is_empty(), "{passed:?} reached the evaluator");
        assert_eq!(command.get_current_dir(), Some(Path::new("/")));
    }

    #[test]
    fn a_root_without_a_manifest_is_refused_before_evaluating() {
        let root = tempfile::tempdir().unwrap();
        match producer().produce(root.path(), None) {
            Err(ProduceError::NoManifest(_)) => {}
            other => panic!("expected the missing manifest, got {other:?}"),
        }
    }

    #[test]
    fn batches_cover_every_name_in_order_four_to_a_core() {
        let names: Vec<String> = (0..100).map(|i| format!("p{i:03}")).collect();
        let split = batches(projects(&names), 4);
        assert_eq!(split.len(), 15);
        assert!(split.iter().all(|batch| batch.len() <= 7));
        assert_eq!(split.concat(), projects(&names));
        assert_eq!(batches(Vec::new(), 4), Vec::<Vec<Project>>::new());
    }

    // a failure evaluates its batch's earlier projects again, so a repository's size mustn't grow
    // what one failure costs.
    #[test]
    fn no_batch_holds_more_projects_than_a_failure_should_cost() {
        let names: Vec<String> = (0..10_000).map(|i| format!("p{i:05}")).collect();
        let split = batches(projects(&names), 2);
        assert!(split.iter().all(|batch| batch.len() <= BATCH_PROJECTS));
        assert_eq!(split.len(), 10_000usize.div_ceil(BATCH_PROJECTS));
        assert_eq!(split.concat(), projects(&names));
    }

    #[test]
    fn no_batch_asks_for_more_than_an_argument_can_hold() {
        let names: Vec<String> = (0..64).map(|i| format!("{i:0>4000}")).collect();
        let split = batches(projects(&names), 1);
        for batch in &split {
            let request = Request::Evaluate(batch).to_json();
            assert!(request.len() <= REQUEST_BYTES, "{}", request.len());
        }
        assert_eq!(split.concat(), projects(&names));
    }

    /// how a project of the scripted repository evaluates.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Behaves {
        Evaluates,
        /// fails, with a frame that names the project.
        Throws,
        /// fails with no frame that names it, as a crash would.
        Crashes,
        /// fails only as its text is written out, naming its field when asked to locate it.
        InvalidUtf8,
    }

    /// a repository the producer's evaluator stands in for: what each project does, what it
    /// depends on, and every evaluation the producer asked for.
    struct Scripted {
        projects: BTreeMap<String, (Behaves, Vec<String>)>,
        /// each project's toolchain imports, by the owner each names.
        imports: BTreeMap<String, Vec<String>>,
        /// the systems the manifest's catalog names, if it declares one.
        catalog: Option<Vec<&'static str>>,
        catalog_fails: bool,
        manifest_fails: bool,
        garbage: bool,
        /// a project whose evaluation can't even be started.
        unrunnable: Option<&'static str>,
        /// how long each evaluation takes, so a test can see evaluations overlap.
        takes: std::time::Duration,
        running: AtomicUsize,
        most_running: AtomicUsize,
        requests: Mutex<Vec<String>>,
    }

    impl Scripted {
        fn new(projects: &[(&str, Behaves, &[&str])]) -> Self {
            Self {
                projects: projects
                    .iter()
                    .map(|(name, behaves, depends)| {
                        let depends = depends.iter().map(|d| (*d).to_owned()).collect();
                        ((*name).to_owned(), (*behaves, depends))
                    })
                    .collect(),
                imports: BTreeMap::new(),
                catalog: None,
                catalog_fails: false,
                manifest_fails: false,
                garbage: false,
                unrunnable: None,
                takes: std::time::Duration::ZERO,
                running: AtomicUsize::new(0),
                most_running: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn behaves(&self, name: &str) -> Behaves {
            self.projects[name].0
        }

        fn answer(answer: serde_json::Value) -> Printed {
            Printed::Answer(
                serde_json::json!({
                    "schema": "shikumi.evaluation",
                    "version": {"major": 1, "minor": 0},
                    "answer": answer
                })
                .to_string()
                .into_bytes(),
            )
        }

        // an error as every implementation prints it under `--show-trace`.
        fn error(project: Option<&str>, frames: &[String], message: &str) -> Printed {
            let mut text = String::from("error:\n");
            if let Some(project) = project {
                text.push_str(&format!("       … shikumi-project {project:?}\n\n"));
            }
            for frame in frames {
                text.push_str(&format!("       … {frame}\n\n"));
            }
            text.push_str(&format!("       error: {message}\n"));
            Printed::Error(text)
        }

        fn thrown(name: &str) -> Printed {
            Self::error(
                Some(name),
                &[format!(
                    "while calling the 'throw' builtin\n         at /repo/{name}/project.nix:3:5:"
                )],
                &format!("{name} threw"),
            )
        }

        fn utf8(name: &str, field: bool) -> Printed {
            let mut frames = Vec::new();
            if field {
                frames.push(format!(
                    r#"shikumi-field {{"path":["project","summary"],"at":{{"file":"{name}/project.nix","line":4,"column":5}}}}"#
                ));
            }
            frames.push(
                "while calling the 'toJSON' builtin\n         at /layer/produce.nix:50:7:"
                    .to_owned(),
            );
            Self::error(
                Some(name),
                &frames,
                "JSON serialization error: [json.exception.type_error.316] invalid UTF-8 byte at index 0: 0xC3",
            )
        }

        // a declaration whose `project.dependsOn` lists what the script says it depends on.
        fn declaration(&self, name: &str) -> serde_json::Value {
            let depends: Vec<_> = self.projects[name]
                .1
                .iter()
                .map(|d| serde_json::json!({"kind": "string", "value": d, "context": false}))
                .collect();
            let string = |value: &str| serde_json::json!({"kind": "string", "value": value, "context": false});
            let imports: serde_json::Map<String, serde_json::Value> = self
                .imports
                .get(name)
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, owner)| {
                    (
                        format!("t{index}"),
                        serde_json::json!({"at": null, "value": string(owner)}),
                    )
                })
                .collect();
            serde_json::json!({"factory": true, "value": {"kind": "attrs", "fields": {
                "project": {"at": null, "value": {"kind": "attrs", "fields": {
                    "dependsOn": {"at": null, "value": {"kind": "list", "items": depends}},
                    "imports": {"at": null, "value": {"kind": "attrs", "fields": imports}}
                }}}
            }}})
        }

        fn respond(&self, request: &Request<'_>) -> Printed {
            if self.garbage {
                return Printed::Answer(b"{}".to_vec());
            }
            match request {
                Request::Plan {} => {
                    if self.manifest_fails {
                        return Self::error(
                            None,
                            &["from call site\n         at /repo/shikumi.nix:2:3:".to_owned()],
                            "the manifest failed",
                        );
                    }
                    let files: BTreeMap<&String, String> = self
                        .projects
                        .keys()
                        .map(|name| (name, format!("{name}/project.nix")))
                        .collect();
                    let catalog = self.catalog.as_ref().map(|systems| {
                        serde_json::json!({"file": "catalog/catalog.nix", "systems": systems})
                    });
                    Self::answer(serde_json::json!({"plan": {
                        "manifest": {"file": "shikumi.nix", "value": {"kind": "null"}},
                        "files": files,
                        "catalog": catalog
                    }}))
                }
                Request::Evaluate(names) => {
                    for name in names.iter().map(|project| project.name.as_str()) {
                        match self.behaves(name) {
                            Behaves::Evaluates => {}
                            Behaves::Throws => return Self::thrown(name),
                            Behaves::Crashes => return Printed::Error(String::new()),
                            Behaves::InvalidUtf8 => return Self::utf8(name, false),
                        }
                    }
                    let declarations: Vec<_> = names
                        .iter()
                        .map(|project| self.declaration(&project.name))
                        .collect();
                    Self::answer(serde_json::json!({ "projects": declarations }))
                }
                Request::Catalog { system, .. } => {
                    if self.catalog_fails {
                        return Self::error(
                            None,
                            &["while calling the 'throw' builtin\n         at /repo/catalog/catalog.nix:4:5:"
                                .to_owned()],
                            "the catalog threw",
                        );
                    }
                    if *system == "unknown-system" {
                        return Self::answer(serde_json::json!({"catalog": null}));
                    }
                    Self::answer(serde_json::json!({"catalog": {
                        "value": {"kind": "attrs", "fields": {}},
                        "packages": {},
                        "graph": {}
                    }}))
                }
                Request::Locate(project) => match self.behaves(&project.name) {
                    Behaves::InvalidUtf8 => Self::utf8(&project.name, true),
                    _ => Self::answer(
                        serde_json::json!({"projects": [self.declaration(&project.name)]}),
                    ),
                },
            }
        }
    }

    impl Evaluate for Scripted {
        fn evaluate(&self, request: &Request<'_>) -> Result<Printed, ProduceError> {
            let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_running.fetch_max(running, Ordering::SeqCst);
            self.requests.lock().unwrap().push(label(request));
            if let (Some(name), Request::Evaluate(names)) = (self.unrunnable, request)
                && names.iter().any(|project| project.name == name)
            {
                self.running.fetch_sub(1, Ordering::SeqCst);
                return Err(ProduceError::NoEvaluator);
            }
            std::thread::sleep(self.takes);
            let printed = self.respond(request);
            self.running.fetch_sub(1, Ordering::SeqCst);
            Ok(printed)
        }

        fn places(&self) -> Places<'_> {
            Places {
                root: "/repo",
                layer: "/layer",
                nixpkgs: "/nixpkgs",
            }
        }
    }

    // a request as the tests list it: a plan as its JSON, and the projects the others name.
    fn label(request: &Request<'_>) -> String {
        match request {
            Request::Plan { .. } => request.to_json(),
            Request::Evaluate(projects) => {
                let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
                format!("evaluate {}", names.join(","))
            }
            Request::Locate(project) => format!("locate {}", project.name),
            Request::Catalog { system, .. } => format!("catalog {system}"),
        }
    }

    fn jobs(batches: Vec<Vec<Project>>) -> Vec<Job> {
        batches.into_iter().map(Job::Projects).collect()
    }

    fn entries(declarations: &Declarations) -> BTreeMap<String, String> {
        declarations
            .projects
            .iter()
            .map(|(name, entry)| (name.clone(), serde_json::to_string(entry).unwrap()))
            .collect()
    }

    // each project evaluated alone, in a batch of its own.
    fn alone(scripted: &Scripted) -> BTreeMap<String, String> {
        let names: Vec<String> = scripted.projects.keys().cloned().collect();
        let each: Vec<Vec<Project>> = names.iter().map(|name| vec![project(name)]).collect();
        evaluate(scripted, jobs(each), 1)
            .unwrap()
            .projects
            .into_iter()
            .map(|(name, entry)| (name, serde_json::to_string(&entry).unwrap()))
            .collect()
    }

    // a small deterministic generator, so every run of the test tries the same repositories.
    fn behaviours(seed: u64, count: usize) -> Vec<Behaves> {
        let mut state = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (0..count)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                match (state >> 33) % 10 {
                    0 | 1 => Behaves::Throws,
                    2 => Behaves::Crashes,
                    3 => Behaves::InvalidUtf8,
                    _ => Behaves::Evaluates,
                }
            })
            .collect()
    }

    #[test]
    fn batched_evaluation_gives_what_each_project_gives_alone() {
        for seed in 0..40 {
            let kinds = behaviours(seed, 23);
            let names: Vec<String> = (0..kinds.len()).map(|i| format!("p{i:02}")).collect();
            let projects: Vec<(&str, Behaves, &[&str])> = names
                .iter()
                .zip(&kinds)
                .map(|(name, kind)| (name.as_str(), *kind, &[][..]))
                .collect();
            let scripted = Scripted::new(&projects);
            let expected = alone(&scripted);
            for parallelism in [1, 2, 3, 16] {
                let declarations = compose(&scripted, None, parallelism).unwrap();
                assert_eq!(
                    entries(&declarations),
                    expected,
                    "seed {seed}, {parallelism} at once, {kinds:?}"
                );
            }
        }
    }

    #[test]
    fn a_failed_project_is_its_own_failure_and_its_batch_runs_again_without_it() {
        let scripted = Scripted::new(&[
            ("a", Behaves::Evaluates, &[]),
            ("b", Behaves::Throws, &[]),
            ("c", Behaves::Evaluates, &[]),
        ]);
        let batch = vec![project("a"), project("b"), project("c")];
        let entries: BTreeMap<String, Entry> = evaluate(&scripted, jobs(vec![batch]), 1)
            .unwrap()
            .projects
            .into_iter()
            .collect();
        let Some(Some(Evaluated::Failed(failed))) = entries.get("b") else {
            panic!("{entries:?}");
        };
        assert_eq!(failed.failure.kind, FailureKind::Thrown);
        assert_eq!(
            user_frames(&failed.failure)[0].file.as_str(),
            "b/project.nix"
        );
        for name in ["a", "c"] {
            assert!(
                matches!(entries.get(name), Some(Some(Evaluated::Value(_)))),
                "{name}"
            );
        }
        let requests = scripted.requests.lock().unwrap().clone();
        assert_eq!(requests, ["evaluate a,b,c", "evaluate a", "evaluate c",]);
    }

    #[test]
    fn a_failure_no_frame_places_is_halved_down_to_its_project() {
        let scripted = Scripted::new(&[
            ("a", Behaves::Evaluates, &[]),
            ("b", Behaves::Evaluates, &[]),
            ("c", Behaves::Crashes, &[]),
            ("d", Behaves::Evaluates, &[]),
        ]);
        let declarations = compose(&scripted, None, 1).unwrap();
        let Some(Some(Evaluated::Failed(failed))) = declarations.projects.get("c") else {
            panic!("{:?}", declarations.projects);
        };
        assert_eq!(failed.failure.kind, FailureKind::Crashed);
        assert_eq!(
            declarations
                .projects
                .values()
                .filter(|entry| matches!(entry, Some(Evaluated::Value(_))))
                .count(),
            3
        );
    }

    #[test]
    fn text_that_isnt_utf8_is_located_to_its_field() {
        let scripted = Scripted::new(&[("a", Behaves::InvalidUtf8, &[])]);
        let declarations = compose(&scripted, None, 1).unwrap();
        let Some(Some(Evaluated::Failed(failed))) = declarations.projects.get("a") else {
            panic!("{:?}", declarations.projects);
        };
        assert_eq!(failed.failure.kind, FailureKind::InvalidUtf8);
        let field = failed.failure.field().unwrap();
        assert_eq!(field.at.as_ref().unwrap().file.as_str(), "a/project.nix");
    }

    // in the old single evaluation, a dependency whose declaration couldn't be read took the
    // whole focused composition down with it, the focused project included.
    #[test]
    fn a_focused_composition_keeps_its_project_when_a_dependency_fails() {
        let scripted = Scripted::new(&[
            ("app", Behaves::Evaluates, &["lib", "tool"]),
            ("lib", Behaves::Throws, &["base"]),
            ("tool", Behaves::Crashes, &[]),
            ("base", Behaves::Evaluates, &[]),
            ("other", Behaves::Throws, &[]),
        ]);
        let declarations = compose(&scripted, Some("app"), 2).unwrap();
        let names: Vec<&str> = declarations.projects.keys().map(String::as_str).collect();
        assert_eq!(names, ["app", "lib", "tool"]);
        assert!(matches!(
            declarations.projects["app"],
            Some(Evaluated::Value(_))
        ));
        for failed in ["lib", "tool"] {
            assert!(
                matches!(declarations.projects[failed], Some(Evaluated::Failed(_))),
                "{failed}"
            );
        }
        let requests = scripted.requests.lock().unwrap().clone();
        assert!(
            !requests.iter().any(|request| request.contains("other")),
            "{requests:?}"
        );
    }

    // a focused composition evaluates its projects as the global one does, so each comes out the
    // same, failures included.
    #[test]
    fn a_focused_composition_gives_each_project_what_the_global_one_gives() {
        let scripted = Scripted::new(&[
            ("app", Behaves::Evaluates, &["lib", "data", "nowhere"]),
            ("lib", Behaves::Throws, &["base"]),
            ("data", Behaves::InvalidUtf8, &[]),
            ("base", Behaves::Evaluates, &[]),
            ("web", Behaves::Evaluates, &["app"]),
        ]);
        let global = entries(&compose(&scripted, None, 3).unwrap());
        for (focus, selected) in [
            ("web", &["app", "data", "lib", "web"][..]),
            ("app", &["app", "data", "lib"]),
            ("base", &["base"]),
            ("data", &["data"]),
        ] {
            let focused = entries(&compose(&scripted, Some(focus), 3).unwrap());
            let names: Vec<&str> = focused.keys().map(String::as_str).collect();
            assert_eq!(names, selected, "{focus}");
            for (name, entry) in &focused {
                assert_eq!(entry, &global[name], "{name} focused on {focus}");
            }
        }
    }

    #[test]
    fn a_focus_on_an_unlisted_project_composes_nothing() {
        let scripted = Scripted::new(&[("app", Behaves::Evaluates, &[])]);
        let declarations = compose(&scripted, Some("nowhere"), 2).unwrap();
        assert!(declarations.projects.is_empty());
        assert_eq!(scripted.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_manifest_that_fails_leaves_nothing_to_compose() {
        let mut scripted = Scripted::new(&[("a", Behaves::Evaluates, &[])]);
        scripted.manifest_fails = true;
        let declarations = compose(&scripted, None, 4).unwrap();
        let Evaluated::Failed(failed) = &declarations.manifest else {
            panic!("{:?}", declarations.manifest);
        };
        assert_eq!(failed.failure.message, "the manifest failed");
        assert_eq!(user_frames(&failed.failure)[0].file.as_str(), "shikumi.nix");
        assert!(declarations.projects.is_empty());
        assert_eq!(scripted.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn no_more_evaluations_run_at_once_than_the_cores_allowed() {
        let names: Vec<String> = (0..64).map(|i| format!("p{i:02}")).collect();
        let projects: Vec<(&str, Behaves, &[&str])> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let kind = if i % 7 == 0 {
                    Behaves::Throws
                } else {
                    Behaves::Evaluates
                };
                (name.as_str(), kind, &[][..])
            })
            .collect();
        // long enough for evaluations to overlap when they can.
        let overlapping = || {
            let mut scripted = Scripted::new(&projects);
            scripted.takes = std::time::Duration::from_millis(1);
            scripted
        };
        for parallelism in [1, 3] {
            let scripted = overlapping();
            compose(&scripted, None, parallelism).unwrap();
            let most = scripted.most_running.load(Ordering::SeqCst);
            assert!(
                most <= parallelism,
                "{most} ran at once, {parallelism} allowed"
            );
        }
        let scripted = overlapping();
        compose(&scripted, None, 3).unwrap();
        assert!(scripted.most_running.load(Ordering::SeqCst) > 1);
    }

    #[test]
    fn an_answer_outside_the_contract_stops_the_run_with_its_error() {
        let mut scripted = Scripted::new(&[("a", Behaves::Evaluates, &[])]);
        scripted.garbage = true;
        assert!(matches!(
            compose(&scripted, None, 4),
            Err(ProduceError::Answer(_))
        ));
    }

    #[test]
    fn after_an_evaluation_fails_to_run_no_other_batch_starts() {
        let names: Vec<String> = (0..12).map(|i| format!("p{i:02}")).collect();
        let projects: Vec<(&str, Behaves, &[&str])> = names
            .iter()
            .map(|name| (name.as_str(), Behaves::Evaluates, &[][..]))
            .collect();
        let mut scripted = Scripted::new(&projects);
        scripted.unrunnable = Some("p03");
        let each: Vec<Vec<Project>> = names.iter().map(|name| vec![project(name)]).collect();
        assert!(matches!(
            evaluate(&scripted, jobs(each), 1),
            Err(ProduceError::NoEvaluator)
        ));
        let requests = scripted.requests.lock().unwrap().clone();
        assert_eq!(requests.last().unwrap(), "evaluate p03");
        assert_eq!(requests.len(), 4);
    }

    // a package set is the costliest thing a run evaluates, so it's read once per system, whatever
    // the number of batches, and queued ahead of the batches that run beside it.
    #[test]
    fn the_catalog_is_read_once_per_system_first_and_beside_the_projects() {
        let names: Vec<String> = (0..40).map(|i| format!("p{i:02}")).collect();
        let projects: Vec<(&str, Behaves, &[&str])> = names
            .iter()
            .map(|name| (name.as_str(), Behaves::Evaluates, &[][..]))
            .collect();
        let mut scripted = Scripted::new(&projects);
        scripted.catalog = Some(vec!["x86_64-linux", "aarch64-darwin", "x86_64-linux"]);
        // one at a time, so the order requests were made in is the order they were queued in.
        let declarations = compose(&scripted, None, 1).unwrap();
        let requests = scripted.requests.lock().unwrap().clone();
        let catalogs: Vec<&String> = requests
            .iter()
            .filter(|request| request.starts_with("catalog "))
            .collect();
        assert_eq!(catalogs, ["catalog aarch64-darwin", "catalog x86_64-linux"]);
        assert!(
            requests[1..3]
                .iter()
                .all(|request| request.starts_with("catalog ")),
            "{requests:?}"
        );
        let catalog = declarations.catalog.unwrap();
        assert_eq!(catalog.file.unwrap().as_str(), "catalog/catalog.nix");
        let read: Vec<&str> = catalog.systems.keys().map(String::as_str).collect();
        assert_eq!(read, ["aarch64-darwin", "x86_64-linux"]);
        assert!(
            catalog
                .systems
                .values()
                .all(|read| matches!(read, Some(Evaluated::Value(_))))
        );
        assert_eq!(declarations.projects.len(), 40);
    }

    #[test]
    fn a_system_nixpkgs_does_not_know_reads_as_no_package_set() {
        let mut scripted = Scripted::new(&[("a", Behaves::Evaluates, &[])]);
        scripted.catalog = Some(vec!["unknown-system"]);
        let catalog = compose(&scripted, None, 2).unwrap().catalog.unwrap();
        assert_eq!(catalog.systems["unknown-system"], None);
    }

    // no declaration reads a package set, so a catalog that fails costs no project its entry.
    #[test]
    fn a_catalog_that_fails_is_its_own_failure_and_every_project_still_evaluates() {
        let mut scripted = Scripted::new(&[
            ("a", Behaves::Evaluates, &[]),
            ("b", Behaves::Evaluates, &[]),
        ]);
        scripted.catalog = Some(vec!["x86_64-linux"]);
        scripted.catalog_fails = true;
        let declarations = compose(&scripted, None, 2).unwrap();
        let catalog = declarations.catalog.unwrap();
        let Some(Some(Evaluated::Failed(failed))) = catalog.systems.get("x86_64-linux") else {
            panic!("{catalog:?}");
        };
        assert_eq!(failed.failure.kind, FailureKind::Thrown);
        assert_eq!(
            user_frames(&failed.failure)[0].file.as_str(),
            "catalog/catalog.nix"
        );
        assert!(
            declarations
                .projects
                .values()
                .all(|entry| matches!(entry, Some(Evaluated::Value(_))))
        );
    }

    // a focused composition holds the project and the toolchains it names, so it takes each
    // imported toolchain's owner with it the way it takes a dependency, and reads the catalog its
    // packages come from.
    #[test]
    fn a_focused_composition_takes_the_owner_of_each_toolchain_it_imports() {
        let mut scripted = Scripted::new(&[
            ("app", Behaves::Evaluates, &[]),
            ("jvm", Behaves::Evaluates, &["base"]),
            ("base", Behaves::Evaluates, &[]),
            ("other", Behaves::Evaluates, &[]),
        ]);
        scripted
            .imports
            .insert("app".to_owned(), vec!["jvm".to_owned()]);
        scripted.catalog = Some(vec!["x86_64-linux"]);
        let global = entries(&compose(&scripted, None, 2).unwrap());
        let declarations = compose(&scripted, Some("app"), 2).unwrap();
        let focused = entries(&declarations);
        let names: Vec<&str> = focused.keys().map(String::as_str).collect();
        assert_eq!(names, ["app", "base", "jvm"]);
        for (name, entry) in &focused {
            assert_eq!(entry, &global[name], "{name}");
        }
        assert!(declarations.catalog.is_some());
    }
}
