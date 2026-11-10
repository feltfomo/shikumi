//! one run of the producer over one checkout: what Git tracks there, its lock, the cache and the
//! files seen so far, and the parts of a composition, each from the cache or from a copy of its
//! files evaluated against a store database of its own. how parts are keyed, copied and shared
//! between runs is in docs/explanation/architecture.md, under the context cache.
//!
//! the plan's part is evaluated before the others' files are known, since the plan names them.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use shikumi_derivation::{References, Sha256, StoreDir, StoreName};
use shikumi_lock::{Lock, ToolchainPin, ToolchainSource};

use super::command::{Copied, Inputs, Run, SETTINGS, command};
use super::jobs::{
    Entry, Evaluate, Printed, Read, SystemRequest, contexts, evaluate_planned, plan,
};
use super::{ProduceError, Produced, Producer, utf8};
use crate::check;
use crate::context::keyed::{Tracked, TrackedError, directory_of};
use crate::context::stat::{Fresh, Seen, StatCache};
use crate::context::tree::{self, Nar};
use crate::context::{
    Cache, CacheError, DocumentKey, Fact, Identity, Key, Part, Registered, Root, Scratch,
    remove_tree, store_directory,
};
use crate::contract::RelativePath;
use crate::declarations::{Catalog, Declarations, Evaluated, Failed, Failure};
use crate::document::Document;
use crate::evaluation::{Plan, PlannedCatalog, Request, SystemContext};
use crate::failure::Places;
use crate::layer::{CATALOG, DECLARATION, MANIFEST, STORE_NAMES};
use crate::toolchain::ToolchainContext;

/// a part of the composition as the cache keeps it: what it came to, and the copy it was evaluated
/// from, whose location its strings can carry.
#[derive(Debug, Serialize, Deserialize)]
struct Kept<T> {
    root: String,
    value: T,
}

/// a set of keyed files, each as seen this run.
type Files = Vec<(String, Seen)>;

/// one run of the producer over one checkout.
pub(super) struct Session<'p> {
    producer: &'p Producer,
    root: PathBuf,
    tracked: Tracked,
    nixpkgs: ToolchainPin,
    identity: Identity,
    cache: Cache,
    stat: StatCache,
    seen: BTreeMap<String, Seen>,
    fresh: Fresh,
    layer: Nar,
    /// the layer and nixpkgs in the private store, rooted, once a part misses.
    stocked: Option<Stocked>,
    /// the roots this run holds in the private store, released when it ends.
    roots: Vec<Root>,
    /// whether this run evaluated anything, after which the cache is maintained.
    missed: bool,
}

/// the copies every evaluation reads besides its own.
struct Stocked {
    layer: Copied,
    nixpkgs: Copied,
}

/// a composition whose plan is read and whose parts are keyed, none of them read yet.
enum Prepared {
    /// the plan reads no declaration, so its failure is the manifest's, and with no manifest
    /// there's nothing to compose.
    Failed {
        focus: Option<String>,
        plan_root: String,
        plan_key: Sha256,
        failure: Failure,
    },
    Planned(Box<Planned>),
}

struct Planned {
    focus: Option<String>,
    plan_root: String,
    plan_key: Sha256,
    plan: Plan,
    projects: Files,
    projects_key: Sha256,
    catalog: Option<CatalogPart>,
}

/// the catalog's part: its file, its keyed files and each system's key.
struct CatalogPart {
    file: RelativePath,
    files: Files,
    keys: Vec<(String, Sha256)>,
}

impl Prepared {
    /// the key the composition's document is kept under, or none for a `shikumi` whose own program
    /// can't be found, which checks every run.
    fn document_key(&self) -> Option<Sha256> {
        let checker = std::env::current_exe()
            .ok()
            .and_then(|program| Identity::of(&program).ok())?;
        let (focus, parts) = match self {
            Self::Failed {
                focus, plan_key, ..
            } => (focus.as_deref(), vec![*plan_key]),
            Self::Planned(planned) => {
                let mut parts = vec![planned.plan_key, planned.projects_key];
                parts.extend(
                    planned
                        .catalog
                        .iter()
                        .flat_map(|catalog| catalog.keys.iter().map(|(_, key)| *key)),
                );
                (planned.focus.as_deref(), parts)
            }
        };
        Some(
            DocumentKey {
                checker: &checker,
                focus,
                parts: &parts,
            }
            .digest(),
        )
    }
}

impl<'p> Session<'p> {
    pub(super) fn open(producer: &'p Producer, root: &Path) -> Result<Self, ProduceError> {
        let root = std::fs::canonicalize(root).map_err(|error| ProduceError::Root {
            root: root.to_owned(),
            error,
        })?;
        if !root.join(MANIFEST.name).is_file() {
            return Err(ProduceError::NoManifest(root));
        }
        let root_text = utf8(&root)?;
        let tracked = Tracked::list(&root)?;
        if !tracked.has(MANIFEST.name) {
            return Err(TrackedError::Untracked(root).into());
        }
        let lock = Lock::read(&root, tracked.lock())?;
        let nixpkgs = lock.required(ToolchainSource::Nixpkgs)?.clone();
        let identity =
            Identity::of(producer.nix_instantiate()).map_err(|error| ProduceError::Spawn {
                program: producer.nix_instantiate().to_owned(),
                error,
            })?;
        let cache = Cache::open(&identity)?;
        let stat = cache.stat(&root_text);
        let layer = tree::directory(producer.layer()).map_err(|error| ProduceError::Read {
            path: producer.layer().to_owned(),
            error,
        })?;
        Ok(Self {
            producer,
            root,
            tracked,
            nixpkgs,
            identity,
            cache,
            stat,
            seen: BTreeMap::new(),
            fresh: Fresh::default(),
            layer,
            stocked: None,
            roots: Vec::new(),
            missed: false,
        })
    }

    /// saves what was hashed afresh, releases what the run held and, after a miss, maintains the
    /// cache.
    pub(super) fn close(self) {
        let Self {
            tracked,
            cache,
            stat,
            fresh,
            roots,
            missed,
            identity,
            ..
        } = self;
        stat.save(|file| tracked.has(file), fresh);
        drop(roots);
        if missed {
            cache.maintain(&identity.nix_store());
        }
    }

    /// how each file is seen, statting and hashing only those not seen yet this run.
    fn see(&mut self, files: &[String]) -> Result<Files, ProduceError> {
        let new: Vec<String> = files
            .iter()
            .filter(|file| !self.seen.contains_key(*file))
            .cloned()
            .collect();
        let (seen, fresh) =
            self.stat
                .see(&self.root, &new)
                .map_err(|error| ProduceError::Read {
                    path: self.root.clone(),
                    error,
                })?;
        self.seen.extend(new.into_iter().zip(seen));
        self.fresh.extend(fresh);
        Ok(files
            .iter()
            .map(|file| (file.clone(), self.seen[file]))
            .collect())
    }

    fn key(&self, files: &[(String, Seen)], part: Part<'_>) -> Sha256 {
        Key {
            evaluator: &self.identity,
            settings: SETTINGS,
            layer: &self.layer.hash,
            nixpkgs: &self.nixpkgs.nar_hash,
            files,
            part,
        }
        .digest()
    }

    /// the plan's part: the manifest, and every file the plan tests for by name.
    fn plan_files(&self) -> Vec<String> {
        let mut files = vec![MANIFEST.name.to_owned()];
        files.extend(self.tracked.named(&[DECLARATION.name, CATALOG.name]));
        files.sort_unstable();
        files.dedup();
        files
    }

    /// the plan, from the cache or a copy of its part, and its key.
    fn plan(&mut self) -> Result<(Kept<Result<Plan, Failure>>, Sha256), ProduceError> {
        let files = self.see(&self.plan_files())?;
        let key = self.key(&files, Part::Plan);
        if let Some(kept) = self.cache.get(&key) {
            return Ok((kept, key));
        }
        let [copied] = self.copy([&files])?;
        let evaluator = self.evaluator(copied)?;
        let kept = Kept {
            root: evaluator.inputs.root.path.clone(),
            value: plan(&evaluator)?,
        };
        self.cache.put(&key, &kept);
        Ok((kept, key))
    }

    /// the catalog's part of a plan that names a catalog file: its keyed files, and the key of each
    /// system's `read`.
    fn catalog_part(
        &mut self,
        plan: &Plan,
        read: SystemRequest,
    ) -> Result<Option<CatalogPart>, ProduceError> {
        let Some(PlannedCatalog {
            file: Some(file),
            systems,
        }) = &plan.catalog
        else {
            return Ok(None);
        };
        let directory = directory_of(file.as_str()).map(str::to_owned);
        let files = self.see(&self.tracked.keyed(directory.as_deref()))?;
        let mut systems: Vec<&String> = systems.iter().collect();
        systems.sort_unstable();
        systems.dedup();
        let keys = systems
            .into_iter()
            .map(|system| {
                let part = match read {
                    SystemRequest::Catalog => Part::Catalog {
                        file: file.as_str(),
                        system,
                    },
                    SystemRequest::Context => Part::Context {
                        file: file.as_str(),
                        system,
                    },
                };
                (system.clone(), self.key(&files, part))
            })
            .collect();
        Ok(Some(CatalogPart {
            file: file.clone(),
            files,
            keys,
        }))
    }

    /// the plan and every part's key, without reading any part.
    fn prepare(&mut self, focus: Option<&str>) -> Result<Prepared, ProduceError> {
        let (planned, plan_key) = self.plan()?;
        let plan = match planned.value {
            Ok(plan) => plan,
            Err(failure) => {
                return Ok(Prepared::Failed {
                    focus: focus.map(str::to_owned),
                    plan_root: planned.root,
                    plan_key,
                    failure,
                });
            }
        };
        // the projects' copy holds the manifest and every project's and the catalog's directory;
        // the catalog's holds its directory alone, so editing a declaration leaves its key alone.
        let catalog_directory = plan
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.file.as_ref())
            .and_then(|file| directory_of(file.as_str()));
        let directories: Vec<&str> = plan
            .files
            .values()
            .flatten()
            .filter_map(|file| directory_of(file.as_str()))
            .chain(catalog_directory)
            .collect();
        let mut files = vec![MANIFEST.name.to_owned()];
        files.extend(self.tracked.keyed(directories));
        files.sort_unstable();
        let projects = self.see(&files)?;
        let projects_key = self.key(&projects, Part::Projects { focus });
        let catalog = self.catalog_part(&plan, SystemRequest::Catalog)?;
        Ok(Prepared::Planned(Box::new(Planned {
            focus: focus.map(str::to_owned),
            plan_root: planned.root,
            plan_key,
            plan,
            projects,
            projects_key,
            catalog,
        })))
    }

    /// the document, from the cache, or checked from the declarations and kept.
    pub(super) fn document(
        &mut self,
        focus: Option<&str>,
        parallelism: usize,
    ) -> Result<Document, ProduceError> {
        let prepared = self.prepare(focus)?;
        let key = prepared.document_key();
        if let Some(document) = key.and_then(|key| self.cache.get::<Document>(&key)) {
            return Ok(document);
        }
        let produced = self.finish(prepared, parallelism)?;
        let roots: Vec<&str> = produced.roots.iter().map(String::as_str).collect();
        let document = check::check(&produced.declarations, &roots).map_err(ProduceError::Check)?;
        if let Some(key) = key {
            self.cache.put(&key, &document);
        }
        Ok(document)
    }

    pub(super) fn declarations(
        &mut self,
        focus: Option<&str>,
        parallelism: usize,
    ) -> Result<Produced, ProduceError> {
        let prepared = self.prepare(focus)?;
        self.finish(prepared, parallelism)
    }

    /// the declarations of a prepared composition: each part from the cache, and every part that
    /// misses evaluated from its copy and kept.
    fn finish(&mut self, prepared: Prepared, parallelism: usize) -> Result<Produced, ProduceError> {
        // where Lix reads the private store, under this machine's cache directory, which the
        // location a declaration sees there starts with.
        let store = utf8(self.cache.store())?;
        let planned = match prepared {
            Prepared::Failed {
                focus,
                plan_root,
                failure,
                ..
            } => {
                return Ok(Produced {
                    declarations: Declarations::new(
                        focus,
                        Evaluated::Failed(Failed { failure }),
                        None,
                        BTreeMap::new(),
                    ),
                    roots: vec![plan_root, store],
                });
            }
            Prepared::Planned(planned) => *planned,
        };
        let focus = planned.focus.as_deref();
        let mut projects: Option<Kept<BTreeMap<String, Entry>>> =
            self.cache.get(&planned.projects_key);
        let mut catalogs: BTreeMap<String, Read> = BTreeMap::new();
        let mut catalog_root = None;
        let mut missing = Vec::new();
        for (system, key) in planned.catalog.iter().flat_map(|catalog| &catalog.keys) {
            match self.cache.get::<Kept<Read>>(key) {
                Some(kept) => {
                    catalog_root = Some(kept.root);
                    catalogs.insert(system.clone(), kept.value);
                }
                None => missing.push((system.clone(), *key)),
            }
        }

        let catalog = planned.catalog.as_ref().filter(|_| !missing.is_empty());
        let (projects_copy, catalog_copy) = match (&projects, catalog) {
            (Some(_), None) => (None, None),
            (None, None) => {
                let [projects] = self.copy([&planned.projects])?;
                (Some(projects), None)
            }
            (Some(_), Some(catalog)) => {
                let [catalog] = self.copy([&catalog.files])?;
                (None, Some(catalog))
            }
            (None, Some(catalog)) => {
                let [projects, catalog] = self.copy([&planned.projects, &catalog.files])?;
                (Some(projects), Some(catalog))
            }
        };
        if projects_copy.is_some() || catalog_copy.is_some() {
            let projects_evaluator = projects_copy
                .map(|copied| self.evaluator(copied))
                .transpose()?;
            let catalog_evaluator = catalog_copy
                .map(|copied| self.evaluator(copied))
                .transpose()?;
            let systems: Vec<String> = missing.iter().map(|(system, _)| system.clone()).collect();
            let composed = evaluate_planned(
                &planned.plan,
                focus,
                parallelism,
                projects_evaluator.as_ref(),
                catalog_evaluator
                    .as_ref()
                    .map(|evaluator| (evaluator, systems.as_slice())),
            )?;
            if let Some(evaluator) = projects_evaluator {
                let kept = Kept {
                    root: evaluator.inputs.root.path,
                    value: composed.projects,
                };
                self.cache.put(&planned.projects_key, &kept);
                projects = Some(kept);
            }
            if let Some(evaluator) = catalog_evaluator {
                for (system, key) in &missing {
                    let read = composed.catalogs.get(system).cloned().flatten();
                    let kept = Kept {
                        root: evaluator.inputs.root.path.clone(),
                        value: read.clone(),
                    };
                    self.cache.put(key, &kept);
                    catalogs.insert(system.clone(), read);
                }
                catalog_root = Some(evaluator.inputs.root.path);
            }
        }
        let projects = projects.expect("the projects are cached or evaluated");
        let mut roots = vec![planned.plan_root, store, projects.root];
        roots.extend(catalog_root);
        let catalog = planned.plan.catalog.map(|catalog| Catalog {
            file: catalog.file,
            systems: catalogs,
        });
        Ok(Produced {
            declarations: Declarations::new(
                planned.focus,
                Evaluated::Value(planned.plan.manifest),
                catalog,
                projects.value,
            ),
            roots,
        })
    }

    /// the toolchain context of every system the catalog is read for, from the cache or from a
    /// copy of the catalog's part, its systems evaluated side by side.
    pub(super) fn context(&mut self, parallelism: usize) -> Result<ToolchainContext, ProduceError> {
        let (planned, _) = self.plan()?;
        let plan = planned.value.map_err(|_| ProduceError::NoCatalog)?;
        let catalog = self
            .catalog_part(&plan, SystemRequest::Context)?
            .ok_or(ProduceError::NoCatalog)?;
        let mut systems = BTreeMap::new();
        let mut missing = Vec::new();
        for (system, key) in &catalog.keys {
            match self.cache.get::<Kept<Option<SystemContext>>>(key) {
                Some(kept) => {
                    systems.insert(system.clone(), kept.value);
                }
                None => missing.push((system.clone(), *key)),
            }
        }
        if !missing.is_empty() {
            let [copied] = self.copy([&catalog.files])?;
            let evaluator = self.evaluator(copied)?;
            let names: Vec<String> = missing.iter().map(|(system, _)| system.clone()).collect();
            let mut evaluated = contexts(&evaluator, &catalog.file, &names, parallelism)?;
            for (system, key) in missing {
                let context = evaluated
                    .remove(&system)
                    .expect("every system asked for is answered")
                    .map_err(|failure| ProduceError::Context {
                        system: system.clone(),
                        failure: Box::new(failure),
                    })?;
                self.cache.put(
                    &key,
                    &Kept {
                        root: evaluator.inputs.root.path.clone(),
                        value: context.clone(),
                    },
                );
                systems.insert(system, context);
            }
        }
        Ok(ToolchainContext::new(systems))
    }

    /// the evaluator over `copied`, against a database of its own over the private store that
    /// registers only the layer, nixpkgs and that copy.
    fn evaluator(&self, copied: Copied) -> Result<Evaluator<'_>, ProduceError> {
        let stocked = self
            .stocked
            .as_ref()
            .expect("a copy stocks the store first");
        let registered = [&stocked.layer, &stocked.nixpkgs, &copied].map(|copy| Registered {
            path: copy.path.clone(),
            nar: copy.nar,
        });
        let nix_store = self.identity.nix_store();
        let (scratch, store) =
            self.cache
                .evaluation_store(self.cache.store(), &nix_store, &registered)?;
        let inputs = Inputs {
            store,
            layer: stocked.layer.clone(),
            nixpkgs: stocked.nixpkgs.clone(),
            root: copied,
        };
        let twin = if self.reads_at_real_location(&inputs)? {
            let twin = self.cache.twin();
            let paths =
                [&inputs.layer.path, &inputs.nixpkgs.path, &inputs.root.path].map(String::as_str);
            twin.mirror(self.cache.store(), &paths)?;
            let (scratch, store) =
                self.cache
                    .evaluation_store(twin.root(), &nix_store, &registered)?;
            Some(Twinned {
                inputs: Inputs {
                    store,
                    ..inputs.clone()
                },
                root: utf8(twin.root())?,
                _store: scratch,
            })
        } else {
            None
        };
        Ok(Evaluator {
            nix_instantiate: self.producer.nix_instantiate(),
            cache: &self.cache,
            inputs,
            _store: scratch,
            twin,
        })
    }

    /// whether this evaluator reads a store at its real location, so a declaration's `./.` is where
    /// the copy lies on disk: asked of the evaluator once, with `inputs`, and kept with its identity.
    fn reads_at_real_location(&self, inputs: &Inputs) -> Result<bool, ProduceError> {
        let key = Fact::ReadsAtRealLocation {
            evaluator: &self.identity,
        }
        .digest();
        if let Some(known) = self.cache.get::<bool>(&key) {
            return Ok(known);
        }
        let program = self.producer.nix_instantiate();
        let output = command(program, &self.cache, &Run::Locate { inputs })
            .output()
            .map_err(|error| ProduceError::Spawn {
                program: program.to_owned(),
                error,
            })?;
        if !output.status.success() {
            return Err(store_failed(&self.cache, &output.stderr));
        }
        let located: String = serde_json::from_slice(&output.stdout)
            .map_err(|error| ProduceError::Copy(error.to_string()))?;
        let real = located.starts_with(&utf8(self.cache.store())?);
        self.cache.put(&key, &real);
        Ok(real)
    }

    /// the copies of `parts`, each a set of keyed files, in the private store, rooted for the rest
    /// of the run, with the layer and nixpkgs the first time a run needs them.
    fn copy<const N: usize>(
        &mut self,
        parts: [&[(String, Seen)]; N],
    ) -> Result<[Copied; N], ProduceError> {
        self.missed = true;
        // no collection runs from before the first root until every copy has landed. each root
        // names a path that isn't valid until its copy lands, and a collection skips such a root
        // (cppnix 2.35.3 src/libstore/gc.cc:217-228, determinate 3.24.2 gc.cc:296-305, lix 2.95.4
        // and 2.96.0 lix/libstore/gc.cc:263-273); the copying evaluation's own temporary root ends
        // when it exits. under this hold a collection either ends before the roots, or starts
        // after the copies and finds every root valid.
        let _held = self.cache.between_collections()?;
        let stock = match self.stocked {
            Some(_) => None,
            None => Some(Stock {
                layer: Wanted {
                    source: self.producer.layer().to_owned(),
                    name: STORE_NAMES.layer,
                    copied: self.copied(STORE_NAMES.layer, self.layer)?,
                },
                nixpkgs: self.nixpkgs_wanted()?,
            }),
        };
        let mut staged = Vec::with_capacity(N);
        for part in parts {
            let (scratch, nar) = self.stage(part)?;
            let copied = self.copied(STORE_NAMES.keyed, nar)?;
            staged.push((scratch, copied));
        }
        if let Some(stock) = &stock {
            self.copy_into_store(&[&stock.layer, &stock.nixpkgs])?;
        }
        self.place(&staged)?;
        if let Some(Stock { layer, nixpkgs }) = stock {
            self.stocked = Some(Stocked {
                layer: layer.copied,
                nixpkgs: self.stocked_nixpkgs(nixpkgs.copied.path)?,
            });
        }
        let copies: Vec<Copied> = staged.into_iter().map(|(_, copied)| copied).collect();
        Ok(copies
            .try_into()
            .expect("staging makes one copy of each part"))
    }

    /// puts each staged keyed copy in the private store and registers it there, as a copying
    /// evaluation would leave it. the staged tree is the copy, hashed already from the bytes
    /// staging read, so having an evaluation read, hash and write every file again is work the
    /// run would wait on for nothing: the tree is made canonical and renamed into place, each part
    /// on Rayon's pool, and the store's database loads every registration at once. runs placing
    /// the same path take turns under its copy lock.
    ///
    /// a path counts as there only when the store's database holds it as valid. a tree on disk at
    /// one it doesn't is what an interrupted collection or a cleaned database leaves, and a valid
    /// path is never hashed again, so registering it would have evaluation trust whatever it
    /// holds. such a tree is deleted and the copy placed, as Nix's own add deletes it (cppnix
    /// 2.35.3 libstore/local-store.cc:1266-1278, determinate 3.24.2 1562-1577, lix 2.95.4
    /// 1302-1313, lix 2.96.0 1312-1323).
    fn place(&self, staged: &[(Scratch, Copied)]) -> Result<(), ProduceError> {
        if staged.is_empty() {
            return Ok(());
        }
        let paths: Vec<&str> = staged
            .iter()
            .map(|(_, copied)| copied.path.as_str())
            .collect();
        let _copying = self.cache.copying(&paths)?;
        let nix_store = self.identity.nix_store();
        let invalid = self.cache.invalid(&nix_store, &paths)?;
        let placing: Vec<&(Scratch, Copied)> = staged
            .iter()
            .filter(|(_, copied)| invalid.contains(&copied.path))
            .collect();
        let directory = store_directory(self.cache.store());
        placing.par_iter().try_for_each(|(scratch, copied)| {
            let name = Path::new(&copied.path)
                .file_name()
                .expect("a store path has a name");
            let target = directory.join(name);
            if std::fs::symlink_metadata(&target).is_ok() {
                remove_tree(&target).map_err(self.writing("deleting an unregistered", &target))?;
            }
            // a directory moves to another parent only while it's writable, since its `..`
            // changes; so what it holds is made canonical first, and the directory itself once
            // it's in place.
            within(scratch.path(), canonical).map_err(self.writing("placing", scratch.path()))?;
            std::fs::rename(scratch.path(), &target).map_err(self.writing("placing", &target))?;
            canonical(&target).map_err(self.writing("placing", &target))
        })?;
        let registered: Vec<Registered> = placing
            .iter()
            .map(|(_, copied)| Registered {
                path: copied.path.clone(),
                nar: copied.nar,
            })
            .collect();
        self.cache.register(&nix_store, &registered)?;
        Ok(())
    }

    /// what a failed write into Shikumi's cache was: the cache's disk out of space, or otherwise the
    /// write that failed and where.
    fn writing(&self, action: &'static str, path: &Path) -> impl FnOnce(io::Error) -> ProduceError {
        let cache = self.cache.dir().to_owned();
        let path = path.to_owned();
        move |error| {
            if error.kind() == io::ErrorKind::StorageFull {
                ProduceError::DiskFull { cache }
            } else {
                CacheError::Io {
                    action,
                    path,
                    error,
                }
                .into()
            }
        }
    }

    /// the copy named `name` of a tree with `nar`, at the path `shikumi-derivation` computes for
    /// it, which the conformance checks hold to every implementation's, rooted from before it's
    /// added until the run ends.
    fn copied(&mut self, name: &str, nar: Nar) -> Result<Copied, ProduceError> {
        let store = StoreDir::default();
        let name = StoreName::new(name).expect("the store names are valid");
        let path = store.display(
            store
                .source_address(&name, nar.hash, References::default())
                .expect("a copy refers to nothing")
                .path(),
        );
        self.roots.push(self.cache.root(&path)?);
        Ok(Copied { path, nar })
    }

    /// stages `part`'s files, each as the key hashed it, and the root marker, and hashes the tree
    /// from the bytes in hand.
    fn stage(&self, part: &[(String, Seen)]) -> Result<(Scratch, Nar), ProduceError> {
        let scratch = self.cache.scratch("stage")?;
        let mut files: Vec<(&str, bool, Vec<u8>)> = part
            .par_iter()
            .filter_map(|(file, seen)| match seen {
                Seen::File { digest, executable } => Some((file, digest, *executable)),
                Seen::Absent => None,
            })
            .map(|(file, digest, executable)| {
                let path = self.root.join(file);
                let bytes = std::fs::read(&path).map_err(|error| ProduceError::Read {
                    path: path.clone(),
                    error,
                })?;
                if Sha256::digest(&bytes) != *digest {
                    return Err(ProduceError::Changed(file.clone()));
                }
                self.write_staged(&scratch.path().join(file), executable, &bytes)?;
                Ok((file.as_str(), executable, bytes))
            })
            .collect::<Result<_, _>>()?;
        let marker = b"./.\n".to_vec();
        self.write_staged(
            &scratch.path().join(STORE_NAMES.root_marker),
            false,
            &marker,
        )?;
        files.push((STORE_NAMES.root_marker, false, marker));
        let nar = tree::files(
            files
                .iter()
                .map(|(file, executable, bytes)| (*file, *executable, bytes.as_slice())),
        )
        .map_err(self.writing("staging", scratch.path()))?;
        Ok((scratch, nar))
    }

    fn write_staged(
        &self,
        path: &Path,
        executable: bool,
        bytes: &[u8],
    ) -> Result<(), ProduceError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(self.writing("staging", parent))?;
        }
        std::fs::write(path, bytes).map_err(self.writing("staging", path))?;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode))
            .map_err(self.writing("staging", path))
    }

    /// runs the copying evaluation over `wanted`, which may read only their sources, and checks
    /// each copy landed at the path computed for it.
    fn copy_into_store(&self, wanted: &[&Wanted]) -> Result<(), ProduceError> {
        #[derive(Serialize)]
        struct Listed<'a> {
            source: &'a Path,
            name: &'a str,
            hash: String,
        }
        let listed: Vec<Listed<'_>> = wanted
            .iter()
            .map(|wanted| Listed {
                source: &wanted.source,
                name: wanted.name,
                hash: wanted.copied.nar.hash.to_sri(),
            })
            .collect();
        let scratch = self.cache.scratch("copies")?;
        let file = scratch.path().join("copies.json");
        let bytes = serde_json::to_vec(&listed).expect("the copies serialize");
        std::fs::write(&file, bytes).map_err(self.writing("listing copies in", &file))?;
        let sources: Vec<&Path> = wanted
            .iter()
            .map(|wanted| wanted.source.as_path())
            .collect();
        let landing: Vec<&str> = wanted
            .iter()
            .map(|wanted| wanted.copied.path.as_str())
            .collect();
        let _copying = self.cache.copying(&landing)?;
        let output = command(
            self.producer.nix_instantiate(),
            &self.cache,
            &Run::Copy {
                copies: &file,
                sources: &sources,
            },
        )
        .output()
        .map_err(|error| ProduceError::Spawn {
            program: self.producer.nix_instantiate().to_owned(),
            error,
        })?;
        if !output.status.success() {
            return Err(store_failed(&self.cache, &output.stderr));
        }
        let paths: Vec<String> = serde_json::from_slice(&output.stdout)
            .map_err(|error| ProduceError::Copy(error.to_string()))?;
        for (wanted, path) in wanted.iter().zip(&paths) {
            if *path != wanted.copied.path {
                return Err(ProduceError::Copy(format!(
                    "{} landed at {path}, not at the path Shikumi computed for it",
                    wanted.copied.path
                )));
            }
        }
        Ok(())
    }

    /// the locked nixpkgs as the copying evaluation takes it, rooted: from the host's store when
    /// the tree is there, as it is once anything on the host has fetched it, and otherwise fetched
    /// straight into the private store first, where the copy finds it already valid.
    fn nixpkgs_wanted(&mut self) -> Result<Wanted, ProduceError> {
        let store = StoreDir::default();
        let path = store.display(&self.nixpkgs.store_path(&store));
        self.roots.push(self.cache.root(&path)?);
        if !Path::new(&path).is_dir() {
            let _copying = self.cache.copying(&[&path])?;
            self.fetch_nixpkgs(&self.cache.store_url(None))?;
        }
        Ok(Wanted {
            source: PathBuf::from(&path),
            name: shikumi_lock::SOURCE_NAME,
            copied: Copied {
                path,
                // the size is the store's to say once the tree is there.
                nar: Nar {
                    hash: *self.nixpkgs.nar_hash.sha256(),
                    size: 0,
                },
            },
        })
    }

    /// the copied nixpkgs with its NAR's size, pinned in the private store. the size follows from
    /// the NAR hash, so the store is asked for it once and the answer kept.
    fn stocked_nixpkgs(&self, path: String) -> Result<Copied, ProduceError> {
        let key = Fact::NarSize {
            nar: &self.nixpkgs.nar_hash,
        }
        .digest();
        let size = match self.cache.get::<u64>(&key) {
            Some(size) => size,
            None => {
                let size = self
                    .cache
                    .command(&self.identity.nix_store())
                    .arg("--store")
                    .arg(self.cache.store_url(None))
                    .args(["--query", "--size"])
                    .arg(&path)
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| String::from_utf8(output.stdout).ok())
                    .and_then(|size| size.trim().parse().ok())
                    .ok_or_else(|| {
                        ProduceError::Copy(format!("{path} isn't in Shikumi's store"))
                    })?;
                self.cache.put(&key, &size);
                size
            }
        };
        self.cache.pin(&path)?;
        Ok(Copied {
            path,
            nar: Nar {
                hash: *self.nixpkgs.nar_hash.sha256(),
                size,
            },
        })
    }

    fn fetch_nixpkgs(&self, store: &OsString) -> Result<(), ProduceError> {
        let program = self.identity.sibling("nix-prefetch-url");
        let output = self
            .cache
            .command(&program)
            .args(["--unpack", "--type", "sha256", "--name"])
            .arg(shikumi_lock::SOURCE_NAME)
            .args(["--option", "store"])
            .arg(store)
            .arg(self.nixpkgs.url.as_str())
            .arg(self.nixpkgs.nar_hash.sha256().to_base32())
            .output()
            .map_err(|error| ProduceError::Spawn {
                program: program.clone(),
                error,
            })?;
        if output.status.success() {
            Ok(())
        } else if full(&output.stderr) {
            Err(ProduceError::DiskFull {
                cache: self.cache.dir().to_owned(),
            })
        } else {
            Err(ProduceError::Fetch {
                url: self.nixpkgs.url.as_str().to_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }
}

/// a copy to make: where it's taken from, its store name, and where it lands.
struct Wanted {
    source: PathBuf,
    name: &'static str,
    copied: Copied,
}

/// the copies a run's first miss makes besides its own: the layer and the locked nixpkgs.
struct Stock {
    layer: Wanted,
    nixpkgs: Wanted,
}

/// `entry` as Nix leaves an entry of a store copy: not writable, executable where its owner could
/// execute it or it's a directory, and its mtime one second after the epoch (cppnix 2.35.3
/// src/libstore/posix-fs-canonicalise.cc:13-34, determinate 3.24.2 13-34, lix 2.95.4
/// lix/libstore/local-store.cc:465-502, lix 2.96.0 475-512).
fn canonical(entry: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(entry)?;
    let mode = if metadata.is_dir() || metadata.permissions().mode() & 0o100 != 0 {
        0o555
    } else {
        0o444
    };
    std::fs::File::open(entry)?
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1))?;
    std::fs::set_permissions(entry, std::fs::Permissions::from_mode(mode))
}

/// `apply` to everything under `directory`, each directory after what it holds, which a read-only
/// directory would refuse otherwise.
fn within(directory: &Path, apply: fn(&Path) -> io::Result<()>) -> io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            within(&entry.path(), apply)?;
        }
        apply(&entry.path())?;
    }
    Ok(())
}

/// the evaluator's text with the private store's real location written as the store directory it
/// stands for, which is how every other path in it is printed, and anything else under Shikumi's
/// cache directory named as that, so no finding carries where this machine keeps it.
fn normalized(cache: &Cache, text: &str) -> String {
    let real = store_directory(cache.store()).display().to_string();
    text.replace(&real, StoreDir::default().as_str())
        .replace(&cache.dir().display().to_string(), CACHE_NAME)
}

/// what an evaluation that writes into the private store failing with `stderr` was: the disk the
/// cache lies on running out of space, or otherwise a failure of Shikumi's own.
fn store_failed(cache: &Cache, stderr: &[u8]) -> ProduceError {
    if full(stderr) {
        ProduceError::DiskFull {
            cache: cache.dir().to_owned(),
        }
    } else {
        ProduceError::Copy(normalized(cache, &String::from_utf8_lossy(stderr)))
    }
}

/// whether an evaluator's error is a write that found no space: `ENOSPC` as `strerror` gives it on
/// Linux and macOS.
fn full(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).contains("No space left on device")
}

/// `text` with every `from` written as `to`.
fn replaced(text: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.windows(from.len()).position(|window| window == from) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(to);
        rest = &rest[at + from.len()..];
    }
    out.extend_from_slice(rest);
    out
}

/// what a finding calls Shikumi's cache directory in place of where it is.
const CACHE_NAME: &str = "«shikumi-cache»";

/// the evaluator over one set of copies.
pub(super) struct Evaluator<'s> {
    nix_instantiate: &'s Path,
    cache: &'s Cache,
    inputs: Inputs,
    /// the evaluation's own database, removed when the evaluator is.
    _store: Scratch,
    /// for an evaluator that reads a store at its real location, the same copies under the store's
    /// twin, which every request is evaluated against too.
    twin: Option<Twinned>,
}

/// the inputs of an evaluation against the store's twin.
struct Twinned {
    inputs: Inputs,
    /// the twin's root, which its evaluation prints where the other prints the store's.
    root: String,
    _store: Scratch,
}

impl Evaluator<'_> {
    fn run(&self, inputs: &Inputs, request: &Request<'_>) -> Result<Output, ProduceError> {
        command(
            self.nix_instantiate,
            self.cache,
            &Run::Evaluate { inputs, request },
        )
        .output()
        .map_err(|error| ProduceError::Spawn {
            program: self.nix_instantiate.to_owned(),
            error,
        })
    }
}

impl Evaluate for Evaluator<'_> {
    /// an evaluator with a twin evaluates each request under the store and its twin at once, the
    /// twin's on a scoped thread of the job's own, so waiting on the two children holds no thread
    /// another job or an answer's reading needs. with the twin's root written as the store's, the
    /// two print the same unless the declarations hold a value built from where the copy lies.
    fn evaluate(&self, request: &Request<'_>) -> Result<Printed, ProduceError> {
        let output = match &self.twin {
            None => self.run(&self.inputs, request)?,
            Some(twin) => {
                let (here, there) = std::thread::scope(|scope| {
                    let there = scope.spawn(|| self.run(&twin.inputs, request));
                    let here = self.run(&self.inputs, request);
                    (
                        here,
                        there
                            .join()
                            .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
                    )
                });
                let (here, there) = (here?, there?);
                let store = utf8(self.cache.store())?;
                let moved = |text: &[u8]| replaced(text, twin.root.as_bytes(), store.as_bytes());
                if here.status.success() != there.status.success()
                    || here.stdout != moved(&there.stdout)
                    || (!here.status.success() && here.stderr != moved(&there.stderr))
                {
                    return Ok(Printed::Moved);
                }
                here
            }
        };
        Ok(if output.status.success() {
            Printed::Answer(output.stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // a signal leaves nothing on stderr to say so; the status does.
            Printed::Error(if stderr.trim().is_empty() {
                format!("the evaluator exited with {}", output.status)
            } else {
                normalized(self.cache, &stderr)
            })
        })
    }

    fn processes(&self) -> usize {
        1 + usize::from(self.twin.is_some())
    }

    fn places(&self) -> Places<'_> {
        Places {
            root: &self.inputs.root.path,
            layer: &self.inputs.layer.path,
            nixpkgs: &self.inputs.nixpkgs.path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(base: &Path) -> Cache {
        let identity = Identity::of(Path::new("/proc/self/exe")).unwrap();
        Cache::open_in(base, base, &identity).unwrap()
    }

    // Lix prints where it reads the private store: a path in it, the store directory itself, and
    // whatever a declaration builds from them. none of it reaches a finding as where this machine
    // keeps its cache.
    #[test]
    fn no_finding_text_names_the_cache_directory() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        let store = opened.store().display().to_string();
        let text = format!(
            "access to absolute path '{store}/nix/store/aaaa-shikumi-keyed/x.nix' is forbidden; \
             reading '{store}/nix/store'; thrown at {store}"
        );
        let shown = normalized(&opened, &text);
        assert!(
            !shown.contains(&base.path().display().to_string()),
            "{shown}"
        );
        assert!(
            shown.contains("'/nix/store/aaaa-shikumi-keyed/x.nix'")
                && shown.contains("'/nix/store'"),
            "{shown}"
        );
    }

    // a copy that runs out of space says the disk is full and where the cache is, not that
    // Shikumi failed.
    #[test]
    fn a_copy_without_space_is_a_full_disk() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        let stderr = b"error: writing to file: No space left on device";
        match store_failed(&opened, stderr) {
            ProduceError::DiskFull { cache } => assert_eq!(cache, opened.dir()),
            other => panic!("a copy without space came back as {other:?}"),
        }
        assert!(matches!(
            store_failed(&opened, b"error: something else"),
            ProduceError::Copy(_)
        ));
    }

    // the twin's evaluation prints the twin's root where the store's prints the store's.
    #[test]
    fn replacing_writes_every_occurrence() {
        assert_eq!(replaced(b"a/t/x a/t/y", b"a/t", b"s"), b"s/x s/y");
        assert_eq!(replaced(b"none", b"a/t", b"s"), b"none");
    }

    // a placed tree is a store copy's: nothing writable, every mtime Nix's, executables still
    // executable, and the directory itself last, once it's in place.
    #[test]
    fn a_placed_tree_is_canonical_throughout() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let base = tempfile::tempdir().unwrap();
        let tree = base.path().join("tree");
        std::fs::create_dir_all(tree.join("a/b")).unwrap();
        std::fs::write(tree.join("a/b/c.nix"), "1").unwrap();
        std::fs::write(tree.join("x.sh"), "2").unwrap();
        std::fs::set_permissions(tree.join("x.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        within(&tree, canonical).unwrap();
        canonical(&tree).unwrap();
        for (path, mode) in [
            ("", 0o555),
            ("a", 0o555),
            ("a/b", 0o555),
            ("a/b/c.nix", 0o444),
            ("x.sh", 0o555),
        ] {
            let metadata = std::fs::symlink_metadata(tree.join(path)).unwrap();
            assert_eq!(metadata.mode() & 0o777, mode, "{path}");
            assert_eq!(metadata.mtime(), 1, "{path}");
        }
        remove_tree(&tree).unwrap();
    }
}
