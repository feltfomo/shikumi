//! running a composition's evaluations: each project's declaration in batches, the catalog once per
//! system and the toolchain context once per system, every job against the evaluator of the copy it
//! reads, as many evaluator processes at once as the process has cores, and each answer read on
//! Rayon's pool as it arrives. how a failure is isolated to its project, what that costs, and how
//! the evaluator processes end with the run are in docs/explanation/architecture.md, under the
//! declaration layer.
//!
//! a project comes out exactly as it would evaluated alone, however batches split around a failure,
//! only because the evaluator is pure and a failed evaluation leaves nothing behind; the batching
//! leans on that and nothing else.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Condvar, Mutex, PoisonError};

use super::ProduceError;
use crate::contract::RelativePath;
use crate::declarations::{
    CatalogRead, Declaration, Evaluated, Failed, Failure, FailureKind, Node, Origin, UserOrigin,
};
use crate::evaluation::{
    Answer, Evaluation, Plan, PlannedCatalog, Project, Request, SystemContext,
};
use crate::failure::{Classified, Places, classify};
use crate::vocabulary::names;

/// how many batches each core gets, so a batch of slow declarations doesn't leave the other cores
/// idle at the end of the run.
const BATCHES_PER_CORE: usize = 4;

/// the most projects one batch holds. a failure evaluates again the projects its batch evaluated
/// before it, so this bounds what each failure costs whatever the repository's size; a batch this
/// size costs a few times what starting its evaluator does.
const BATCH_PROJECTS: usize = 64;

/// the most bytes of project names one batch's request carries. a command-line argument can't be
/// longer than 128 KiB on Linux, `MAX_ARG_STRLEN` in include/uapi/linux/binfmts.h since Linux
/// 2.6.23.
const REQUEST_BYTES: usize = 64 * 1024;

/// what one evaluation printed: its answer, or the error it failed with; or, for an evaluator that
/// reads a store at its real location, that what it printed under the store and under the store's
/// twin differ, so what was evaluated depends on where the copy lies.
#[derive(Debug)]
pub(super) enum Printed {
    Answer(Vec<u8>),
    Error(String),
    Moved,
}

/// the failure of a declaration whose value depends on where the evaluator read its copy.
fn moved() -> Failure {
    Failure {
        kind: FailureKind::Location,
        message: "it evaluates differently where the copy lies elsewhere".to_owned(),
        origin: Origin::User(UserOrigin {
            frames: Vec::new(),
            field: None,
        }),
    }
}

/// one evaluation of the declaration layer against one set of copies. the producer's evaluator runs
/// `nix-instantiate`; the tests' stands in for it.
pub(super) trait Evaluate: Sync {
    fn evaluate(&self, request: &Request<'_>) -> Result<Printed, ProduceError>;
    /// how many evaluator processes one evaluation runs at once.
    fn processes(&self) -> usize {
        1
    }
    /// where the files its evaluations read lie.
    fn places(&self) -> Places<'_>;
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

/// the plan, or the manifest's failure: the plan reads no declaration, so its failure is the
/// manifest's.
pub(super) fn plan(evaluator: &impl Evaluate) -> Result<Result<Plan, Failure>, ProduceError> {
    let request = Request::Plan {};
    Ok(match evaluator.evaluate(&request)? {
        Printed::Answer(printed) => match answer(&request, &printed)? {
            Answer::Plan(plan) => Ok(plan),
            _ => unreachable!("an answer is held to its request's shape"),
        },
        Printed::Error(stderr) => {
            let Classified { failure, .. } = classify(&stderr, evaluator.places());
            Err(failure)
        }
        Printed::Moved => Err(moved()),
    })
}

/// what the parts of a planned composition this run evaluated came to.
#[derive(Debug, Default)]
pub(super) struct Composed {
    pub(super) projects: BTreeMap<String, Entry>,
    pub(super) catalogs: BTreeMap<String, Read>,
}

/// evaluates the parts of a planned composition that have an evaluator: every project the
/// composition holds, against `projects`, and the catalog for each of `systems`, against `catalog`.
pub(super) fn evaluate_planned<E: Evaluate>(
    plan: &Plan,
    focus: Option<&str>,
    parallelism: usize,
    projects: Option<&E>,
    catalog: Option<(&E, &[String])>,
) -> Result<Composed, ProduceError> {
    // the catalog is read once per system, beside the first round's projects; a focused
    // composition reads it whole too, since its shell's packages come from it.
    let mut catalog_jobs: Vec<Job<'_, E>> = match (&plan.catalog, catalog) {
        (
            Some(PlannedCatalog {
                file: Some(file), ..
            }),
            Some((evaluator, systems)),
        ) => systems
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<&str>>()
            .into_iter()
            .map(|system| Job::Catalog {
                file: file.clone(),
                system: system.to_owned(),
                evaluator,
            })
            .collect(),
        _ => Vec::new(),
    };

    let mut composed = Composed::default();
    let mut next: BTreeSet<&str> = match (projects, focus) {
        (None, _) => BTreeSet::new(),
        (Some(_), None) => plan.files.keys().map(String::as_str).collect(),
        (Some(_), Some(focus)) => plan
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
                    composed.projects.insert(name.to_owned(), None);
                }
            }
        }
        // the catalog's evaluations go first: each reads a whole package set, the slowest
        // evaluations of the run.
        let mut jobs = std::mem::take(&mut catalog_jobs);
        if let Some(evaluator) = projects {
            jobs.extend(
                batches(pending, parallelism)
                    .into_iter()
                    .map(|batch| Job::Projects { batch, evaluator }),
            );
        }
        let evaluated = evaluate(jobs, parallelism)?;
        composed.catalogs.extend(evaluated.catalogs);
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
        composed.projects.extend(evaluated.projects);
        next.retain(|name| !composed.projects.contains_key(*name));
    }
    Ok(composed)
}

/// the toolchain context of the catalog in `file`, for each of `systems`, side by side.
pub(super) fn contexts<E: Evaluate>(
    evaluator: &E,
    file: &RelativePath,
    systems: &[String],
    parallelism: usize,
) -> Result<BTreeMap<String, Result<Option<SystemContext>, Failure>>, ProduceError> {
    let jobs = systems
        .iter()
        .map(|system| Job::Context {
            file: file.clone(),
            system: system.clone(),
            evaluator,
        })
        .collect();
    Ok(evaluate(jobs, parallelism)?.contexts.into_iter().collect())
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
pub(super) type Entry = Option<Evaluated<Declaration>>;

/// what the catalog came to on one system: null for a system the pinned nixpkgs doesn't know.
pub(super) type Read = Option<Evaluated<CatalogRead>>;

/// one evaluation to run, against the evaluator of the copy it reads: a batch of projects, or the
/// catalog or the toolchain context for one system.
enum Job<'e, E> {
    Projects {
        batch: Vec<Project>,
        evaluator: &'e E,
    },
    Catalog {
        file: RelativePath,
        system: String,
        evaluator: &'e E,
    },
    Context {
        file: RelativePath,
        system: String,
        evaluator: &'e E,
    },
}

impl<E: Evaluate> Job<'_, E> {
    fn processes(&self) -> usize {
        match self {
            Self::Projects { evaluator, .. }
            | Self::Catalog { evaluator, .. }
            | Self::Context { evaluator, .. } => evaluator.processes(),
        }
    }
}

/// what a round of evaluations came to.
#[derive(Debug, Default)]
struct Results {
    projects: Vec<(String, Entry)>,
    catalogs: Vec<(String, Read)>,
    contexts: Vec<(String, Result<Option<SystemContext>, Failure>)>,
}

/// the work the evaluating threads share: the jobs still to run, how many are running, what came
/// back, and the first error, after which no job starts.
struct Shared<'e, E> {
    queue: VecDeque<Job<'e, E>>,
    running: usize,
    results: Results,
    error: Option<ProduceError>,
}

/// runs every job, with at most `parallelism` evaluator processes at a time, and returns what each
/// came to. each job's thread starts and waits for its own processes, so the threads number
/// `parallelism` over the processes a job runs at once.
fn evaluate<E: Evaluate>(
    jobs: Vec<Job<'_, E>>,
    parallelism: usize,
) -> Result<Results, ProduceError> {
    let processes = jobs.iter().map(Job::processes).max().unwrap_or(1);
    let workers = (parallelism / processes).max(1).min(jobs.len());
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
                threads.spawn(|| work(&shared, &changed, reads));
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

fn work<'s, 'e: 's, E: Evaluate>(
    shared: &'s Mutex<Shared<'e, E>>,
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
        let outcome = run(job);
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
            Ok(Ran::Read {
                request,
                file,
                system,
                printed,
            }) => reads.spawn(move |_| {
                let read = read_system(request, &file, &system, &printed);
                let mut state = lock();
                match read {
                    Ok(SystemAnswer::Catalog(read)) => state
                        .results
                        .catalogs
                        .push((system, read.map(Evaluated::Value))),
                    Ok(SystemAnswer::Context(context)) => {
                        state.results.contexts.push((system, Ok(context)));
                    }
                    Err(error) => {
                        state.error.get_or_insert(error);
                    }
                }
                drop(state);
                changed.notify_all();
            }),
            Ok(Ran::SystemFailed {
                request,
                system,
                failure,
            }) => match request {
                SystemRequest::Catalog => state
                    .results
                    .catalogs
                    .push((system, Some(Evaluated::Failed(Failed { failure })))),
                SystemRequest::Context => state.results.contexts.push((system, Err(failure))),
            },
            Ok(Ran::Failed {
                project,
                failure,
                again,
            }) => {
                state
                    .results
                    .projects
                    .push((project, Some(Evaluated::Failed(Failed { failure }))));
                state.queue.extend(again);
            }
            Ok(Ran::Split(halves)) => state.queue.extend(halves),
            Err(error) => {
                state.error.get_or_insert(error);
            }
        }
        drop(state);
        changed.notify_all();
    }
}

/// which of the two requests read one system's package set.
#[derive(Debug, Clone, Copy)]
pub(super) enum SystemRequest {
    Catalog,
    Context,
}

impl SystemRequest {
    fn request<'r>(self, file: &'r RelativePath, system: &'r str) -> Request<'r> {
        match self {
            Self::Catalog => Request::Catalog { file, system },
            Self::Context => Request::Context { file, system },
        }
    }
}

/// what a system's evaluation answered, read.
enum SystemAnswer {
    Catalog(Option<CatalogRead>),
    Context(Option<SystemContext>),
}

/// what running one job came to: a batch's answer, still to be read, or the project it failed in
/// and the batches left to evaluate without it; or one system's answer, or its failure.
enum Ran<'e, E> {
    Answered(Vec<Project>, Vec<u8>),
    Read {
        request: SystemRequest,
        file: RelativePath,
        system: String,
        printed: Vec<u8>,
    },
    /// a system's evaluation failed. it reads nothing but the catalog, so the failure is the
    /// catalog's wherever its frames lie.
    SystemFailed {
        request: SystemRequest,
        system: String,
        failure: Failure,
    },
    Failed {
        project: String,
        failure: Failure,
        again: Vec<Job<'e, E>>,
    },
    /// a failure no frame placed: the batch, halved, to evaluate again.
    Split(Vec<Job<'e, E>>),
}

fn run<E: Evaluate>(job: Job<'_, E>) -> Result<Ran<'_, E>, ProduceError> {
    let (kind, file, system, evaluator) = match job {
        Job::Projects { batch, evaluator } => return run_batch(evaluator, batch),
        Job::Catalog {
            file,
            system,
            evaluator,
        } => (SystemRequest::Catalog, file, system, evaluator),
        Job::Context {
            file,
            system,
            evaluator,
        } => (SystemRequest::Context, file, system, evaluator),
    };
    let request = kind.request(&file, &system);
    let printed = evaluator.evaluate(&request)?;
    Ok(match printed {
        Printed::Answer(printed) => Ran::Read {
            request: kind,
            file,
            system,
            printed,
        },
        Printed::Error(stderr) => Ran::SystemFailed {
            request: kind,
            failure: classify(&stderr, evaluator.places()).failure,
            system,
        },
        Printed::Moved => Ran::SystemFailed {
            request: kind,
            failure: moved(),
            system,
        },
    })
}

fn run_batch<E: Evaluate>(evaluator: &E, batch: Vec<Project>) -> Result<Ran<'_, E>, ProduceError> {
    let request = Request::Evaluate(&batch);
    let Classified { project, failure } = match evaluator.evaluate(&request)? {
        Printed::Answer(printed) => return Ok(Ran::Answered(batch, printed)),
        Printed::Error(stderr) => classify(&stderr, evaluator.places()),
        // the evaluations disagree somewhere in the batch, which no frame places.
        Printed::Moved => Classified {
            project: None,
            failure: moved(),
        },
    };
    let again = |batch| Job::Projects { batch, evaluator };
    let at = project
        .as_ref()
        .and_then(|name| batch.iter().position(|project| project.name == *name));
    let Some(at) = at else {
        // a failure no frame places in one project: halve the batch until it's down to the one
        // project it fails with.
        if batch.len() > 1 {
            let mut first = batch;
            let second = first.split_off(first.len() / 2);
            return Ok(Ran::Split(vec![again(first), again(second)]));
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
            .map(again)
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
    let request = Request::Locate(project);
    Ok(match evaluator.evaluate(&request)? {
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
        Printed::Answer(_) | Printed::Moved => failure,
    })
}

fn read_batch(
    projects: Vec<Project>,
    printed: &[u8],
) -> Result<Vec<(String, Entry)>, ProduceError> {
    let declarations = match answer(&Request::Evaluate(&projects), printed)? {
        Answer::Projects(declarations) => declarations,
        _ => unreachable!("an answer is held to its request's shape"),
    };
    Ok(projects
        .into_iter()
        .zip(declarations)
        .map(|(project, declaration)| (project.name, Some(Evaluated::Value(declaration))))
        .collect())
}

fn read_system(
    request: SystemRequest,
    file: &RelativePath,
    system: &str,
    printed: &[u8],
) -> Result<SystemAnswer, ProduceError> {
    Ok(match answer(&request.request(file, system), printed)? {
        Answer::Catalog(read) => SystemAnswer::Catalog(read),
        Answer::Context(context) => SystemAnswer::Context(context),
        Answer::Plan(_) | Answer::Projects(_) => {
            unreachable!("an answer is held to its request's shape")
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::declarations::Declarations;

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
            crate::declarations::Origin::Shikumi(_) => {
                panic!("a user's failure came back as Shikumi's: {failure:?}")
            }
        }
    }

    // batches hold every project once, in order, at least four to a core and none over the most a
    // batch holds.
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
        /// how many processes each evaluation stands for, as one under a store and its twin does.
        processes: usize,
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
                processes: 1,
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

        // an error in the shape `failure.rs` reads, which the failures fixture holds every
        // implementation's `--show-trace` output to.
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
                Request::Context { .. } => {
                    Self::answer(serde_json::json!({"context": {"packages": {}}}))
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

        fn processes(&self) -> usize {
            self.processes
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
            Request::Context { system, .. } => format!("context {system}"),
        }
    }

    fn jobs(scripted: &Scripted, batches: Vec<Vec<Project>>) -> Vec<Job<'_, Scripted>> {
        batches
            .into_iter()
            .map(|batch| Job::Projects {
                batch,
                evaluator: scripted,
            })
            .collect()
    }

    // plans the composition and evaluates every part of it, with nothing cached, as the session
    // does on a cold run.
    fn compose(
        scripted: &Scripted,
        focus: Option<&str>,
        parallelism: usize,
    ) -> Result<Declarations, ProduceError> {
        let plan = match super::plan(scripted)? {
            Ok(plan) => plan,
            Err(failure) => {
                return Ok(Declarations::new(
                    focus.map(str::to_owned),
                    Evaluated::Failed(Failed { failure }),
                    None,
                    BTreeMap::new(),
                ));
            }
        };
        let systems = plan
            .catalog
            .as_ref()
            .map(|catalog| catalog.systems.clone())
            .unwrap_or_default();
        let composed = evaluate_planned(
            &plan,
            focus,
            parallelism,
            Some(scripted),
            Some((scripted, &systems)),
        )?;
        let catalog = plan.catalog.map(|planned| crate::declarations::Catalog {
            file: planned.file,
            systems: composed.catalogs,
        });
        Ok(Declarations::new(
            focus.map(str::to_owned),
            Evaluated::Value(plan.manifest),
            catalog,
            composed.projects,
        ))
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
        evaluate(jobs(scripted, each), 1)
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
        let entries: BTreeMap<String, Entry> = evaluate(jobs(&scripted, vec![batch]), 1)
            .unwrap()
            .projects
            .into_iter()
            .collect();
        let Some(Some(Evaluated::Failed(failed))) = entries.get("b") else {
            panic!("b's failure wasn't kept in its place: {entries:?}");
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
            panic!(
                "c's crash wasn't kept as its failure: {:?}",
                declarations.projects
            );
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
            panic!(
                "a's text that isn't UTF-8 wasn't its failure: {:?}",
                declarations.projects
            );
        };
        assert_eq!(failed.failure.kind, FailureKind::InvalidUtf8);
        let field = failed.failure.field().unwrap();
        assert_eq!(field.at.as_ref().unwrap().file.as_str(), "a/project.nix");
    }

    // a dependency whose declaration can't be read is that dependency's failure; the focused
    // project is still evaluated and checked.
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
            panic!(
                "the manifest's failure wasn't kept: {:?}",
                declarations.manifest
            );
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
        // an evaluator that runs two processes for each evaluation, under a store and its twin,
        // runs half as many evaluations, so its processes still number the cores.
        let mut twinned = overlapping();
        twinned.processes = 2;
        compose(&twinned, None, 4).unwrap();
        let most = twinned.most_running.load(Ordering::SeqCst);
        assert!(
            most <= 2,
            "{most} twinned evaluations ran at once on 4 cores"
        );
        assert!(most > 1, "twinned evaluations ran one at a time on 4 cores");
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
            evaluate(jobs(&scripted, each), 1),
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
            panic!("the catalog's failure wasn't kept for its system: {catalog:?}");
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

    // the toolchain context's systems are evaluated side by side, as the catalog's are, each a
    // whole package set.
    #[test]
    fn every_systems_context_is_evaluated_at_once() {
        let mut scripted = Scripted::new(&[("a", Behaves::Evaluates, &[])]);
        scripted.takes = std::time::Duration::from_millis(20);
        let file = RelativePath::new("catalog/catalog.nix").unwrap();
        let systems = ["x86_64-linux", "aarch64-darwin", "unknown-system"].map(str::to_owned);
        let contexts = contexts(&scripted, &file, &systems, 3).unwrap();
        assert_eq!(scripted.most_running.load(Ordering::SeqCst), 3);
        let read: Vec<&str> = contexts.keys().map(String::as_str).collect();
        assert_eq!(read, ["aarch64-darwin", "unknown-system", "x86_64-linux"]);
        assert!(contexts.values().all(Result::is_ok));
    }
}
