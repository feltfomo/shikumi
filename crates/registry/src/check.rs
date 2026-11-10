//! the checker: the one place a declaration is judged. it reads the serialized manifest, catalog and
//! declarations, checks every value against the manifest's fields, the catalog's record, the
//! declaration record and the repository's extension schemas, applies the rules that span projects
//! and the rules on each shell's packages and variables, and returns the registry document: every
//! project when nothing is wrong, or every finding at once when something is. nothing is thrown;
//! each finding is collected with its code, the field's path and the nearest source position the
//! evaluator gave, so one pass reports all of them. a declaration whose evaluation failed becomes a
//! finding too, placed at the user's own code, and the projects around it are still checked.
//!
//! cost is linear in the size of the declarations: one visit per serialized node, one sort of the
//! manifest's directories for overlaps, one walk of each ordering graph for loops, one map per name
//! space for duplicates and per shell for variables, one lookup per package picked, and per system
//! one visit to each package the shell's picks reach. each project is checked on its own on Rayon's
//! pool, and the findings are put back in name order, so the document is the same however the work
//! was split. a finding's path is a stack pushed and popped on the way down, so locating one copies
//! nothing until the finding is made.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;

use rayon::prelude::*;
use serde_json::{Map, Value};

use crate::contract::{ATTRIBUTE_PATH_ACCEPTS, RelativePath, Scalar, is_attribute_path};
use crate::declarations::{
    At, CatalogRead, Declaration, Declarations, Evaluated, Failure, FailureKind, Field, Manifest,
    Node, Origin,
};
use crate::document::{
    Catalog, Code, Diagnostic, Document, Identifier, Location, Project, Related, Remainder,
    SchemaName, Segment, Severity, ShellSource, Step, Variable, Version, claims, identifier, shell,
    text,
};
use crate::graph::{self, Edge};
use crate::layer::{CATALOG, DECLARATION, MANIFEST};
use crate::vocabulary::{self, Key, ManifestField, Presence, Record, Type, names};

/// how many of the places in the user's code that led to a failure are shown after the one it
/// happened at; a recursion through the user's code can pass a handful of places many times.
const RELATED_FRAMES: usize = 8;
/// how many of Shikumi's own frames a Shikumi bug names.
const INTERNAL_FRAMES: usize = 4;

/// checks the serialized declarations of the repository at `root`, the absolute path the
/// declarations were evaluated from, and builds the document.
pub fn check(declarations: &Declarations, root: &str) -> Result<Document, CheckError> {
    match &declarations.manifest {
        Evaluated::Value(manifest) => Checker::new(declarations, manifest, root).run(),
        // every project's directory is the manifest's to say, so with no manifest there's nothing
        // else to read.
        Evaluated::Failed(failed) => {
            let file = RelativePath::checked(MANIFEST.name.to_owned());
            let (here, path, report) = failure(&file, &failed.failure);
            let found = diagnostic(None, path, here, report);
            Ok(document(
                vec![found],
                None,
                &BTreeMap::new(),
                Vec::new(),
                None,
            ))
        }
    }
}

/// the checker accepted a declaration in a shape the registry document can't hold. the declaration
/// record and the document's types disagree, which is a defect in Shikumi, not in the declaration.
#[derive(Debug)]
pub struct CheckError {
    /// what was accepted: a project, by name, or the catalog.
    subject: String,
    error: serde_json::Error,
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the checker accepted {} in a shape the registry document can't hold, which is a defect in Shikumi: {}",
            self.subject, self.error
        )
    }
}

impl std::error::Error for CheckError {}

struct Checker<'a> {
    declarations: &'a Declarations,
    manifest: &'a Manifest,
    root: &'a str,
    findings: Vec<Diagnostic>,
    /// the project the findings being made belong to.
    project: Option<Identifier>,
    path: Vec<Segment>,
    /// where the repository's one overlay is, for a finding about a second one.
    overlay: Option<Location>,
}

/// where a value sits: the nearest attribute with a position, or the file it was read from.
#[derive(Clone, Copy)]
enum Here<'a> {
    At(&'a At),
    File(&'a RelativePath),
}

impl<'h> Here<'h> {
    fn location(self) -> Location {
        match self {
            Self::At(at) => at.location(),
            Self::File(file) => Location {
                file: file.clone(),
                position: None,
            },
        }
    }

    // a field's own position when it has one, else where its parent was.
    fn within(self, field: &'h Field) -> Self {
        field.at.as_ref().map_or(self, Here::At)
    }
}

/// a finding on its way into the collection.
struct Finding {
    code: Code,
    message: String,
    help: Option<String>,
    related: Vec<Related>,
}

fn finding(code: Code, message: impl Into<String>) -> Finding {
    Finding {
        code,
        message: message.into(),
        help: None,
        related: Vec::new(),
    }
}

impl Finding {
    fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    fn related(mut self, location: Location, message: impl Into<String>) -> Self {
        self.related.push(Related {
            location,
            message: text(message),
        });
        self
    }
}

/// what one listed project came to, for the rules across projects.
struct Checked<'a> {
    name: Identifier,
    file: RelativePath,
    /// the declaration as written, for locating findings in it, and as accepted, when it had no
    /// finding of its own.
    accepted: Option<(&'a Node, Project)>,
}

/// the name spaces in which a name has one owner across the composition: commands share the
/// shell's path, services are started by name, and a toolchain is imported by name.
#[derive(Clone, Copy)]
enum Shared {
    Commands,
    Services,
    Toolchains,
}

impl Shared {
    fn field(self) -> &'static str {
        match self {
            Self::Commands => names::COMMANDS,
            Self::Services => names::SERVICES,
            Self::Toolchains => names::TOOLCHAINS,
        }
    }

    fn names(self, declared: &Project) -> Box<dyn Iterator<Item = &Identifier> + '_> {
        match self {
            Self::Commands => Box::new(declared.commands.keys()),
            Self::Services => Box::new(declared.services.keys()),
            Self::Toolchains => Box::new(declared.toolchains.keys()),
        }
    }

    fn help(self, entry: &str, first: &Identifier) -> String {
        match self {
            Self::Commands | Self::Services => "give each one a name of its own".to_owned(),
            Self::Toolchains => format!(
                "a toolchain has one owner; use `{first}`'s instead: `{}.{}.{entry} = \"{first}\";`",
                names::PROJECT,
                names::IMPORTS,
            ),
        }
    }
}

/// what the manifest says, as far as it says it right.
struct Manifested {
    directories: Directories,
    extensions: BTreeMap<String, Type>,
    /// the catalog the manifest declares: absent when it declares none, unread when its entry has a
    /// finding of its own.
    catalog: Declares<Value>,
}

enum Declares<T> {
    Nothing,
    Wrongly,
    Rightly(T),
}

/// what the catalog came to, for the projects that pick from it.
enum Cataloged<'a> {
    /// the manifest declares no catalog.
    Absent,
    /// the catalog can't say what it holds: its manifest entry has a finding, its file is missing,
    /// or it failed to evaluate. nothing is checked against it, so its own finding stands alone.
    Unread,
    Read(Box<CatalogState<'a>>),
}

struct CatalogState<'a> {
    /// every name the catalog declares under `packages`, whatever its finding.
    names: BTreeSet<&'a str>,
    /// each system that evaluated, in the manifest's order, with what it read.
    reads: Vec<(&'a str, &'a CatalogRead)>,
    /// where the catalog's overlay is, for a finding that points at it.
    overlay: Location,
    /// where the catalog lists its packages, for a pick it doesn't hold.
    listing: Location,
    /// the catalog as the document carries it, when it had no finding.
    accepted: Option<Catalog>,
}

/// one source of a shell, with the declaration it's located in.
struct Source<'c, 'a> {
    project: &'c Checked<'a>,
    declaration: &'a Node,
    shell: ShellSource<'c>,
}

impl<'c, 'a: 'c> Source<'c, 'a> {
    /// how a finding names it.
    fn named(&self) -> String {
        match self.shell.toolchain {
            None => format!("`{}`", self.project.name),
            Some(toolchain) => format!("toolchain `{toolchain}` of `{}`", self.project.name),
        }
    }

    // a variable's path in this source's declaration, and where it's defined.
    fn variable(&self, variable: &Variable) -> (Vec<Segment>, Here<'c>) {
        let mut path = source_prefix(&self.shell);
        path.extend([
            Segment::Field(names::ENV.to_owned()),
            Segment::Field(variable.to_string()),
        ]);
        let here = locate_path(self.declaration, Here::File(&self.project.file), &path);
        (path, here)
    }
}

// where a source sits in its project's declaration: nothing for the project, the toolchain's field
// for a toolchain.
fn source_prefix(source: &ShellSource<'_>) -> Vec<Segment> {
    match source.toolchain {
        None => Vec::new(),
        Some(toolchain) => vec![
            Segment::Field(names::TOOLCHAINS.to_owned()),
            Segment::Field(toolchain.to_string()),
        ],
    }
}

/// the two ways a package goes by a name on a path: the name nixpkgs gives it, and the program it
/// runs as. both share one name space, since a package named `java` and a package that runs as
/// `java` both put `java` on the path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Called<'a> {
    Name(&'a str),
    Program(&'a str),
}

impl<'a> Called<'a> {
    fn name(self) -> &'a str {
        match self {
            Self::Name(name) | Self::Program(name) => name,
        }
    }
}

/// the first package on a path to go by a name, how it goes by it, and how it got there.
struct Claim<'a> {
    called: Called<'a>,
    package: &'a str,
    source: usize,
    index: usize,
    picked: &'a str,
    via: Option<&'a str>,
}

type Directories = BTreeMap<String, Option<RelativePath>>;

impl<'a> Checker<'a> {
    fn new(declarations: &'a Declarations, manifest: &'a Manifest, root: &'a str) -> Self {
        Self {
            declarations,
            manifest,
            root,
            findings: Vec::new(),
            project: None,
            path: Vec::new(),
            overlay: None,
        }
    }

    fn report(&mut self, here: Here<'_>, finding: Finding) {
        let found = diagnostic(self.project.clone(), self.path.clone(), here, finding);
        self.findings.push(found);
    }

    fn run(mut self) -> Result<Document, CheckError> {
        let declarations = self.declarations;
        let manifest = self.manifest;
        let Manifested {
            directories,
            extensions,
            catalog,
        } = self.manifest(&manifest.value, Here::File(&manifest.file));

        self.overlaps(&directories);
        let focus = self.focus(&directories);
        let catalog = self.catalog(catalog)?;
        self.overlay = match &catalog {
            Cataloged::Read(state) => Some(state.overlay.clone()),
            Cataloged::Absent | Cataloged::Unread => None,
        };

        // built once per composition: the repository's extension schemas are the same for every
        // project.
        let record = vocabulary::declaration(&extensions);
        let listed: Vec<(Identifier, &RelativePath, Option<&Evaluated<Declaration>>)> =
            declarations
                .projects
                .iter()
                .filter_map(|(name, declaration)| {
                    let directory = directories.get(name)?.as_ref()?;
                    Some((identifier(name)?, directory, declaration.as_ref()))
                })
                .collect();
        let overlay = &self.overlay;
        // collected in input order, so the findings stay in name order however the work was split.
        let each: Vec<(Vec<Diagnostic>, Checked<'a>)> = listed
            .into_par_iter()
            .map(|(project, directory, declaration)| {
                let mut checker = Checker::new(declarations, manifest, self.root);
                checker.overlay.clone_from(overlay);
                let checked = checker.project(project, directory, declaration, &record)?;
                Ok((checker.findings, checked))
            })
            .collect::<Result<_, CheckError>>()?;
        let (findings, checked): (Vec<_>, Vec<_>) = each.into_iter().unzip();
        self.findings.extend(findings.into_iter().flatten());
        // `checked` is in name order; the index finds a project by name in one lookup.
        let index: HashMap<&str, usize> = checked
            .iter()
            .enumerate()
            .map(|(at, project)| (project.name.as_str(), at))
            .collect();
        let toolchains = self.claims(&checked, Shared::Toolchains);
        for project in &checked {
            self.references(project, &directories);
            self.picks(project, &catalog);
            self.imports(project, &checked, &index, &toolchains, &directories);
        }
        self.dependency_loops(&checked);
        for project in &checked {
            self.step_loops(project);
        }
        for shared in [Shared::Commands, Shared::Services] {
            let claims = self.claims(&checked, shared);
            self.duplicates(&checked, &index, &claims, shared);
        }
        self.duplicates(&checked, &index, &toolchains, Shared::Toolchains);
        let sources = sources(&checked, &index, focus.as_ref());
        self.environment(&sources);
        if let Cataloged::Read(state) = &catalog {
            for (system, read) in &state.reads {
                self.conflicts(&sources, system, read);
            }
        }

        let catalog = match catalog {
            Cataloged::Read(state) => state.accepted,
            Cataloged::Absent | Cataloged::Unread => None,
        };
        Ok(document(
            self.findings,
            focus,
            &directories,
            checked,
            catalog,
        ))
    }

    // the manifest's fields: the listed directories by project name, None where an entry is
    // invalid, the extension schemas that are sound, and the catalog it declares. each field is
    // found in the manifest's one definition, and handled by what it is there.
    fn manifest(&mut self, node: &'a Node, here: Here<'a>) -> Manifested {
        let mut directories = BTreeMap::new();
        let mut extensions = BTreeMap::new();
        let mut catalog = Declares::Nothing;
        let definition = vocabulary::manifest();
        let Some(fields) = self.attribute_set(node, here, "an attribute set") else {
            return Manifested {
                directories,
                extensions,
                catalog: Declares::Wrongly,
            };
        };
        for (name, field) in fields {
            let here = here.within(field);
            self.path.push(Segment::Field(name.clone()));
            match ManifestField::named(name) {
                Some(ManifestField::Projects) => {
                    let Type::Attrs {
                        element: directory, ..
                    } = ManifestField::Projects.field().ty
                    else {
                        unreachable!("the manifest's projects are an attribute set");
                    };
                    if let Some(entries) =
                        self.attribute_set(&field.value, here, "an attribute set")
                    {
                        for (project, entry) in entries {
                            let here = here.within(entry);
                            self.path.push(Segment::Field(project.clone()));
                            self.project = identifier(project);
                            let valid_name = self.key(project, here);
                            let directory = self
                                .value(&directory, &entry.value, here)
                                .filter(|_| valid_name)
                                .and_then(|directory| match directory {
                                    Value::String(directory) => {
                                        Some(RelativePath::checked(directory))
                                    }
                                    _ => None,
                                });
                            directories.insert(project.clone(), directory);
                            self.project = None;
                            self.path.pop();
                        }
                    }
                }
                Some(ManifestField::Extensions) => {
                    if let Some(schemas) =
                        self.attribute_set(&field.value, here, "an attribute set")
                    {
                        for (namespace, schema) in schemas {
                            let here = here.within(schema);
                            self.path.push(Segment::Field(namespace.clone()));
                            if self.key(namespace, here)
                                && let Some(ty) = self.schema(&schema.value, here)
                            {
                                extensions.insert(namespace.clone(), ty);
                            }
                            self.path.pop();
                        }
                    }
                }
                Some(ManifestField::Catalog) => {
                    let entry = ManifestField::Catalog.field().ty;
                    catalog = match self.value(&entry, &field.value, here) {
                        Some(value) => Declares::Rightly(value),
                        None => Declares::Wrongly,
                    };
                }
                None if !self.safe_key(name, here) => {}
                None => self.unknown_option(here, name, definition.names()),
            }
            self.path.pop();
        }
        self.missing(&definition, fields, here);
        Manifested {
            directories,
            extensions,
            catalog,
        }
    }

    // a schema a repository wrote with `shikumi.lib.types`, read as a type.
    fn schema(&mut self, node: &Node, here: Here<'_>) -> Option<Type> {
        let read = Type::from_schema(node);
        if read.is_none() {
            self.report(
                here,
                finding(
                    Code::Invalid,
                    "must be a schema built from `shikumi.lib.types`",
                ),
            );
        }
        read
    }

    // every required field of `record` an attribute set leaves out, each as a finding; whether
    // there was one.
    fn missing(
        &mut self,
        record: &Record,
        fields: &BTreeMap<String, Field>,
        here: Here<'_>,
    ) -> bool {
        let mut any = false;
        for (name, declared) in record.fields() {
            if matches!(declared.presence, Presence::Required) && !fields.contains_key(name) {
                self.path.push(Segment::Field(name.clone()));
                self.report(
                    here,
                    finding(Code::Missing, "is required but not defined")
                        .help(format!("add `{name} = ...;`")),
                );
                self.path.pop();
                any = true;
            }
        }
        any
    }

    // a listed directory at or inside another's. sorted by components, the directories inside one
    // come straight after it, so the directories holding the current one are exactly those still on
    // a stack that drops each directory once the sort has passed everything inside it. one sort and
    // one pass find every overlap, each against the nearest directory that holds it.
    fn overlaps(&mut self, directories: &Directories) {
        let mut sorted: Vec<(Vec<&str>, &String, &RelativePath)> = directories
            .iter()
            .filter_map(|(name, directory)| {
                directory
                    .as_ref()
                    .map(|d| (d.as_str().split('/').collect(), name, d))
            })
            .collect();
        sorted.sort();
        let mut holding: Vec<&(Vec<&str>, &String, &RelativePath)> = Vec::new();
        for listed in &sorted {
            let (inner, inner_name, inner_directory) = listed;
            while holding
                .last()
                .is_some_and(|(outer, ..)| !inner.starts_with(outer))
            {
                holding.pop();
            }
            if let Some((_, outer_name, outer_directory)) = holding.last() {
                self.project = identifier(inner_name);
                self.path = vec![
                    Segment::Field(ManifestField::Projects.name().to_owned()),
                    Segment::Field((*inner_name).clone()),
                ];
                let entry = self.manifest_entry(inner_name);
                let outer_entry = self.manifest_entry(outer_name).location();
                self.report(
                    entry,
                    finding(
                        Code::Overlap,
                        format!(
                            "lists `{inner_directory}`, which lies at or inside `{outer_directory}`, the directory of `{outer_name}`"
                        ),
                    )
                    .help("give every project a directory of its own")
                    .related(outer_entry, format!("`{outer_name}` is listed here")),
                );
            }
            holding.push(listed);
        }
        self.project = None;
        self.path.clear();
    }

    // a focused composition names a listed project; anything else focuses on nothing.
    fn focus(&mut self, directories: &Directories) -> Option<Identifier> {
        let focus = self.declarations.focus.as_deref()?;
        match identifier(focus) {
            Some(project) if directories.contains_key(focus) => Some(project),
            _ => {
                let manifest = Here::File(&self.manifest.file);
                self.report(
                    manifest,
                    finding(
                        Code::UnknownReference,
                        format!("the focused project `{focus}` isn't listed"),
                    ),
                );
                None
            }
        }
    }

    // the catalog: its manifest entry's file and systems, its record, and on each system that
    // read it, what its entries resolve to. the catalog is called with nothing that differs
    // between systems, so its value is the same on each, and the record is checked once.
    fn catalog(&mut self, declared: Declares<Value>) -> Result<Cataloged<'a>, CheckError> {
        let entry = match declared {
            Declares::Nothing => return Ok(Cataloged::Absent),
            Declares::Wrongly => return Ok(Cataloged::Unread),
            Declares::Rightly(entry) => entry,
        };
        let manifest = self.manifest;
        let at = |path: &[&str]| locate(&manifest.value, Here::File(&manifest.file), path);
        let Some(read) = &self.declarations.catalog else {
            return Ok(Cataloged::Unread);
        };
        let Some(file) = read.file.clone() else {
            let directory = entry[names::DIRECTORY].as_str().unwrap_or_default();
            self.path = vec![
                Segment::Field(ManifestField::Catalog.name().to_owned()),
                Segment::Field(names::DIRECTORY.to_owned()),
            ];
            self.report(
                at(&[ManifestField::Catalog.name(), names::DIRECTORY]),
                finding(
                    Code::NoDeclaration,
                    format!("names `{directory}`, which has no {}", CATALOG.name),
                )
                .help(format!("add `{directory}/{}`", CATALOG.name)),
            );
            self.path.clear();
            return Ok(Cataloged::Unread);
        };

        let start = self.findings.len();
        let mut readable = true;
        let mut listed = BTreeSet::new();
        let mut reads: Vec<(&'a str, &'a CatalogRead)> = Vec::new();
        let systems = entry[names::SYSTEMS]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for (index, system) in systems.iter().filter_map(Value::as_str).enumerate() {
            let path = vec![
                Segment::Field(ManifestField::Catalog.name().to_owned()),
                Segment::Field(names::SYSTEMS.to_owned()),
                Segment::Index(index_segment(index)),
            ];
            let here = at(&[ManifestField::Catalog.name(), names::SYSTEMS]);
            if !listed.insert(system) {
                self.path = path;
                self.report(
                    here,
                    finding(Code::Invalid, format!("lists `{system}` again")),
                );
                self.path.clear();
                continue;
            }
            match read.systems.get_key_value(system) {
                // the producer reads every system the manifest names as text.
                None => readable = false,
                Some((_, None)) => {
                    self.path = path;
                    self.report(
                        here,
                        finding(
                            Code::Invalid,
                            format!("names `{system}`, which the pinned nixpkgs doesn't know as a system"),
                        )
                        .help("name systems as nixpkgs does, such as `x86_64-linux` or `aarch64-darwin`"),
                    );
                    self.path.clear();
                }
                Some((_, Some(Evaluated::Failed(failed)))) => {
                    let (here, path, report) = failure(&file, &failed.failure);
                    self.path = path;
                    self.report(here, report);
                    self.path.clear();
                    readable = false;
                }
                Some((system, Some(Evaluated::Value(read)))) => reads.push((system, read)),
            }
        }

        let here = Here::File(&file);
        let mut state = CatalogState {
            overlay: here.location(),
            listing: here.location(),
            names: BTreeSet::new(),
            reads: Vec::new(),
            accepted: None,
        };
        let mut accepted = None;
        if let Some((_, first)) = reads.first() {
            state.overlay = locate(&first.value, here, &[names::OVERRIDES]).location();
            state.listing = locate(&first.value, here, &[names::PACKAGES]).location();
            // a catalog whose `packages` isn't an attribute set says nothing of what it holds,
            // and its own finding says why.
            match field_at(&first.value, &[names::PACKAGES]) {
                Some(Node::Attrs { fields }) => {
                    state.names.extend(fields.keys().map(String::as_str));
                }
                Some(_) => readable = false,
                None => {}
            }
            self.overlay = Some(state.overlay.clone());
            accepted = self.value(&vocabulary::catalog(), &first.value, here);
        }
        for (system, read) in &reads {
            self.packages(read, system, here);
        }
        state.reads = reads;
        if self.findings.len() == start
            && let Some(value) = accepted
        {
            state.accepted = Some(accepted_catalog(value, &entry)?);
        }
        Ok(if readable {
            Cataloged::Read(Box::new(state))
        } else {
            Cataloged::Unread
        })
    }

    // every catalog entry under `packages` names a package on the system, as the read resolved
    // it. one that isn't an attribute path at all is already the record's finding.
    fn packages(&mut self, read: &CatalogRead, system: &str, here: Here<'_>) {
        let Some(Node::Attrs { fields }) = field_at(&read.value, &[names::PACKAGES]) else {
            return;
        };
        for (name, field) in fields {
            let Node::String { value: path, .. } = &field.value else {
                continue;
            };
            if is_attribute_path(path) && !read.packages.contains_key(name) {
                self.path = vec![
                    Segment::Field(names::PACKAGES.to_owned()),
                    Segment::Field(name.clone()),
                ];
                self.report(
                    here.within(field),
                    finding(
                        Code::UnknownReference,
                        format!(
                            "names `{path}`, which is no package in the pinned nixpkgs on {system}"
                        ),
                    )
                    .help(format!(
                        "name a package nixpkgs builds, or add one through the catalog's `{}`",
                        names::OVERRIDES
                    )),
                );
                self.path.clear();
            }
        }
    }

    fn project(
        &mut self,
        name: Identifier,
        directory: &RelativePath,
        declaration: Option<&'a Evaluated<Declaration>>,
        record: &Type,
    ) -> Result<Checked<'a>, CheckError> {
        let file = RelativePath::checked(format!("{directory}/{}", DECLARATION.name));
        let mut checked = Checked {
            name: name.clone(),
            file: file.clone(),
            accepted: None,
        };
        self.project = Some(name.clone());
        match declaration {
            None => {
                self.path = vec![
                    Segment::Field(ManifestField::Projects.name().to_owned()),
                    Segment::Field(name.to_string()),
                ];
                let entry = self.manifest_entry(name.as_str());
                self.report(
                    entry,
                    finding(
                        Code::NoDeclaration,
                        format!("lists `{directory}`, which has no {}", DECLARATION.name),
                    )
                    .help(format!(
                        "add `{file}`, or take `{name}` out of the manifest"
                    )),
                );
                self.path.clear();
            }
            Some(Evaluated::Failed(failed)) => {
                let (here, path, report) = failure(&file, &failed.failure);
                self.path = path;
                self.report(here, report);
                self.path.clear();
            }
            Some(Evaluated::Value(declaration)) if !declaration.factory => {
                let kind = declaration.value.kind();
                // a function that comes back is one that asked for an argument it isn't given.
                let help = if matches!(declaration.value, Node::Function {}) {
                    format!(
                        "a declaration file takes only {}: `{{ shikumi, ... }}: shikumi.project {{ ... }}`",
                        DECLARATION.called_with()
                    )
                } else {
                    "pass the record through the factory: `{ shikumi, ... }: shikumi.project { ... }`"
                        .to_owned()
                };
                self.report(
                    Here::File(&file),
                    finding(
                        Code::NotADeclaration,
                        format!("returns {kind}, not a `shikumi.project` result"),
                    )
                    .help(help),
                );
            }
            Some(Evaluated::Value(declaration)) => {
                if let Some(value) = self.value(record, &declaration.value, Here::File(&file)) {
                    let project = vocabulary::document_value(value, directory);
                    let project = serde_json::from_value(project).map_err(|error| CheckError {
                        subject: format!("`{name}`"),
                        error,
                    })?;
                    checked.accepted = Some((&declaration.value, project));
                }
            }
        }
        self.project = None;
        Ok(checked)
    }

    // a dependency names a listed project, and a step runs after one of its own project's steps.
    fn references(&mut self, project: &Checked<'a>, directories: &Directories) {
        let Some((declaration, declared)) = &project.accepted else {
            return;
        };
        self.project = Some(project.name.clone());
        let file = Here::File(&project.file);
        for (index, dependency) in declared.depends_on.iter().enumerate() {
            if !directories.contains_key(dependency.as_str()) {
                self.reference(
                    declaration,
                    file,
                    &[names::PROJECT, names::DEPENDS_ON],
                    index,
                    finding(
                        Code::UnknownReference,
                        format!("names `{dependency}`, which isn't a listed project"),
                    )
                    .help(format!(
                        "list `{dependency}` in the manifest, or take it out of `{}`",
                        names::DEPENDS_ON
                    )),
                );
            }
        }
        for (step, declared_step) in &declared.steps {
            for (index, other) in declared_step.after.iter().enumerate() {
                if !declared.steps.contains_key(other) {
                    self.reference(
                        declaration,
                        file,
                        &[names::STEPS, step.as_str(), names::AFTER],
                        index,
                        finding(
                            Code::UnknownReference,
                            format!("names `{other}`, which isn't a step of `{}`", project.name),
                        ),
                    );
                }
            }
        }
        self.project = None;
    }

    // every package a project or its toolchains pick, for its path or for a variable, is named
    // in the catalog. a catalog that can't say what it holds checks nothing. a pick's path is
    // built only when it isn't found, which most are.
    fn picks(&mut self, project: &Checked<'a>, catalog: &Cataloged<'_>) {
        let Some((declaration, declared)) = &project.accepted else {
            return;
        };
        let names = match catalog {
            Cataloged::Unread => return,
            Cataloged::Absent => None,
            Cataloged::Read(state) => Some(state),
        };
        let held = |package: &Identifier| {
            names.is_some_and(|state| state.names.contains(package.as_str()))
        };
        let mut missed: Vec<(Vec<Segment>, &Identifier)> = Vec::new();
        for source in declared.sources(&project.name) {
            let prefix = source_prefix(&source);
            for (index, package) in source.packages.iter().enumerate() {
                if !held(package) {
                    let mut path = prefix.clone();
                    path.extend([
                        Segment::Field(names::PACKAGES.to_owned()),
                        Segment::Index(index_segment(index)),
                    ]);
                    missed.push((path, package));
                }
            }
            for (variable, value) in source.env {
                if let Some(package) = value.package().filter(|package| !held(package)) {
                    let mut path = prefix.clone();
                    path.extend([
                        Segment::Field(names::ENV.to_owned()),
                        Segment::Field(variable.to_string()),
                        Segment::Field(names::PACKAGE.to_owned()),
                    ]);
                    missed.push((path, package));
                }
            }
        }
        self.project = Some(project.name.clone());
        for (path, package) in missed {
            let report = match names {
                Some(state) => finding(
                    Code::UnknownReference,
                    format!("names `{package}`, which isn't in the catalog"),
                )
                .help(format!(
                    "add `{package}` to the catalog's `{}`",
                    names::PACKAGES
                ))
                .related(state.listing.clone(), "the catalog's packages are here"),
                None => finding(
                    Code::UnknownReference,
                    format!("names `{package}`, but the repository has no catalog"),
                )
                .help(format!(
                    "declare one in the manifest, `{} = {{ {} = \"...\"; {} = [ ... ]; }};`, and name the package there",
                    ManifestField::Catalog.name(),
                    names::DIRECTORY,
                    names::SYSTEMS,
                )),
            };
            let here = locate_path(declaration, Here::File(&project.file), &path);
            self.path = path;
            self.report(here, report);
        }
        self.path.clear();
        self.project = None;
    }

    // each toolchain a project imports is defined by the project it names. the owner is found by
    // name and who defines a toolchain from the claims the duplicate rule reads too, so the rule
    // costs a lookup per import whatever the number of projects.
    fn imports(
        &mut self,
        project: &Checked<'a>,
        checked: &[Checked<'a>],
        index: &HashMap<&str, usize>,
        toolchains: &BTreeMap<&Identifier, Vec<&Identifier>>,
        directories: &Directories,
    ) {
        let Some((declaration, declared)) = &project.accepted else {
            return;
        };
        self.project = Some(project.name.clone());
        for (toolchain, owner) in &declared.imports {
            self.path = vec![
                Segment::Field(names::PROJECT.to_owned()),
                Segment::Field(names::IMPORTS.to_owned()),
                Segment::Field(toolchain.to_string()),
            ];
            let here = locate_path(declaration, Here::File(&project.file), &self.path);
            if !directories.contains_key(owner.as_str()) {
                self.report(
                    here,
                    finding(
                        Code::UnknownReference,
                        format!("names `{owner}`, which isn't a listed project"),
                    ),
                );
                continue;
            }
            // an owner whose declaration has findings of its own, or one the composition doesn't
            // hold, says nothing of what it defines.
            let Some(owner_project) = index.get(owner.as_str()).map(|&at| &checked[at]) else {
                continue;
            };
            let Some((owner_declaration, owner_declared)) = &owner_project.accepted else {
                continue;
            };
            if owner_declared.toolchains.contains_key(toolchain) {
                continue;
            }
            let help = match toolchains.get(toolchain).and_then(|owners| owners.first()) {
                Some(other) => format!(
                    "`{other}` defines it: `{}.{}.{toolchain} = \"{other}\";`",
                    names::PROJECT,
                    names::IMPORTS,
                ),
                None => format!(
                    "define it in `{owner}`'s `{}.{toolchain}`",
                    names::TOOLCHAINS
                ),
            };
            let at = locate(
                owner_declaration,
                Here::File(&owner_project.file),
                &[names::TOOLCHAINS],
            );
            self.report(
                here,
                finding(
                    Code::UnknownReference,
                    format!("imports `{toolchain}` from `{owner}`, which defines no toolchain `{toolchain}`"),
                )
                .help(help)
                .related(at.location(), format!("`{owner}`'s toolchains are here")),
            );
        }
        self.path.clear();
        self.project = None;
    }

    // a shell gets each environment variable from one place. its first definition, in the order
    // the sources come, keeps it, and each later one is reported against its own declaration;
    // a definition is located only once it's reported or pointed at.
    fn environment(&mut self, sources: &[Source<'_, 'a>]) {
        let mut defined: BTreeMap<&Variable, &Source<'_, 'a>> = BTreeMap::new();
        for source in sources {
            for variable in source.shell.env.keys() {
                let Some(first) = defined.get(variable) else {
                    defined.insert(variable, source);
                    continue;
                };
                let (path, here) = source.variable(variable);
                let at = first.variable(variable).1.location();
                self.project = Some(source.project.name.clone());
                self.path = path;
                self.report(
                    here,
                    finding(
                        Code::Duplicate,
                        format!("is already defined by {}", first.named()),
                    )
                    .help("define each variable once: a project and the toolchains it uses share one environment")
                    .related(at, format!("{} defines it here", first.named())),
                );
            }
        }
        self.path.clear();
        self.project = None;
    }

    // two different packages on one shell's path under one name. the packages each source picks
    // are walked through what they propagate, breadth first in the order picked, and the first
    // package to go by a name, whether as its name or as its program, keeps it; a later, different
    // one is reported against the pick that brought it, pointing at the pick that brought the
    // first. a package reached again is the same package, so each is visited once per system.
    fn conflicts(&mut self, sources: &[Source<'_, 'a>], system: &str, read: &'a CatalogRead) {
        let mut claims: HashMap<&'a str, Claim<'a>> = HashMap::new();
        let mut seen: HashSet<&'a str> = HashSet::new();
        let mut reported: HashSet<(&'a str, &'a str)> = HashSet::new();
        let mut queue: VecDeque<(&'a str, Option<&'a str>)> = VecDeque::new();
        for (index_of_source, source) in sources.iter().enumerate() {
            for (index, picked) in source.shell.packages.iter().enumerate() {
                let Some((picked, root)) = read.packages.get_key_value(picked.as_str()) else {
                    continue;
                };
                queue.push_back((root.as_str(), None));
                while let Some((name, via)) = queue.pop_front() {
                    let Some((name, package)) = read.graph.get_key_value(name) else {
                        continue;
                    };
                    if !seen.insert(name.as_str()) {
                        continue;
                    }
                    let called = [
                        Some(Called::Name(&package.pname)),
                        package.main_program.as_deref().map(Called::Program),
                    ];
                    for called in called.into_iter().flatten() {
                        let claim = Claim {
                            called,
                            package: name,
                            source: index_of_source,
                            index,
                            picked,
                            via,
                        };
                        match claims.entry(called.name()) {
                            Entry::Vacant(vacant) => {
                                vacant.insert(claim);
                            }
                            Entry::Occupied(first) => {
                                let first = first.get();
                                if first.package != name.as_str()
                                    && reported.insert((first.package, name.as_str()))
                                {
                                    self.conflict(sources, system, first, &claim);
                                }
                            }
                        }
                    }
                    queue.extend(
                        package
                            .propagates
                            .iter()
                            .map(|next| (next.as_str(), Some(name.as_str()))),
                    );
                }
            }
        }
        self.path.clear();
        self.project = None;
    }

    fn conflict(
        &mut self,
        sources: &[Source<'_, 'a>],
        system: &str,
        first: &Claim<'_>,
        second: &Claim<'_>,
    ) {
        let how = |claim: &Claim<'_>| match claim.via {
            None => format!("catalog package `{}`", claim.picked),
            Some(via) => format!(
                "catalog package `{}` through the propagated inputs of `{via}`",
                claim.picked
            ),
        };
        let both = match (first.called, second.called) {
            (Called::Name(name), Called::Name(_)) => format!("two packages named `{name}`"),
            (Called::Program(program), Called::Program(_)) => {
                format!("two packages that run as `{program}`")
            }
            (Called::Name(name), Called::Program(_)) | (Called::Program(name), Called::Name(_)) => {
                format!("one package named `{name}` and one that runs as it")
            }
        };
        let at = |claim: &Claim<'_>| {
            let source = &sources[claim.source];
            let mut path = source_prefix(&source.shell);
            path.extend([
                Segment::Field(names::PACKAGES.to_owned()),
                Segment::Index(index_segment(claim.index)),
            ]);
            let here = locate_path(source.declaration, Here::File(&source.project.file), &path);
            (path, here)
        };
        let first_at = at(first).1.location();
        let (path, here) = at(second);
        self.project = Some(sources[second.source].project.name.clone());
        self.path = path;
        let report = finding(
            Code::Conflict,
            format!(
                "puts `{}` on the path, from {}, where `{}` from {} already is: {both} on {system}",
                second.package,
                how(second),
                first.package,
                how(first),
            ),
        )
        .help(format!(
            "keep one of them on the path: pick packages that agree, or make them agree in the catalog's `{}`",
            names::OVERRIDES
        ))
        .related(
            first_at,
            format!("{} puts `{}` on the path here", sources[first.source].named(), first.package),
        );
        self.report(here, report);
    }

    fn reference(
        &mut self,
        declaration: &Node,
        file: Here<'_>,
        path: &[&str],
        index: usize,
        finding: Finding,
    ) {
        let here = locate(declaration, file, path);
        self.path = path
            .iter()
            .map(|segment| Segment::Field((*segment).to_owned()))
            .chain([Segment::Index(index_segment(index))])
            .collect();
        self.report(here, finding);
        self.path.clear();
    }

    // projects that depend on each other in a loop can't be ordered. each loop is reported once,
    // against the dependency its first project in name order takes into it, with every other
    // dependency in the loop as a related location.
    fn dependency_loops(&mut self, checked: &[Checked<'a>]) {
        let accepted: Vec<(&Checked<'a>, &Node, &Project)> = checked
            .iter()
            .filter_map(|project| {
                let (node, declared) = project.accepted.as_ref()?;
                Some((project, *node, declared))
            })
            .collect();
        let position = graph::positions(accepted.iter().map(|(project, ..)| project.name.as_str()));
        let graph: Vec<Vec<Edge>> = accepted
            .iter()
            .map(|(_, _, declared)| {
                graph::edges(
                    declared.depends_on.iter().map(Identifier::as_str),
                    &position,
                )
            })
            .collect();
        for found in graph::cycles(&graph) {
            let names: Vec<&str> = found
                .iter()
                .map(|(node, _)| accepted[*node].0.name.as_str())
                .collect();
            let mut report = finding(
                Code::Cycle,
                format!("makes a loop: {}", graph::sentence(&names, "depends on")),
            )
            .help("take one of these dependencies out, so the projects can be ordered");
            for (node, edge) in &found[1..] {
                let (project, declaration, _) = accepted[*node];
                let at = locate(
                    declaration,
                    Here::File(&project.file),
                    &[names::PROJECT, names::DEPENDS_ON],
                );
                report = report.related(
                    at.location(),
                    format!(
                        "`{}` depends on `{}` here",
                        project.name, accepted[edge.to].0.name
                    ),
                );
            }
            let (first, edge) = found[0];
            let (project, declaration, _) = accepted[first];
            self.project = Some(project.name.clone());
            self.reference(
                declaration,
                Here::File(&project.file),
                &[names::PROJECT, names::DEPENDS_ON],
                edge.at,
                report,
            );
        }
        self.project = None;
    }

    // steps that run after each other in a loop can't be ordered either, reported the same way
    // inside their project.
    fn step_loops(&mut self, project: &Checked<'a>) {
        let Some((declaration, declared)) = &project.accepted else {
            return;
        };
        let steps: Vec<(&Identifier, &Step)> = declared.steps.iter().collect();
        let position = graph::positions(steps.iter().map(|(name, _)| name.as_str()));
        let graph: Vec<Vec<Edge>> = steps
            .iter()
            .map(|(_, step)| graph::edges(step.after.iter().map(Identifier::as_str), &position))
            .collect();
        let file = Here::File(&project.file);
        self.project = Some(project.name.clone());
        for found in graph::cycles(&graph) {
            let names: Vec<&str> = found
                .iter()
                .map(|(node, _)| steps[*node].0.as_str())
                .collect();
            let mut report = finding(
                Code::Cycle,
                format!("makes a loop: {}", graph::sentence(&names, "runs after")),
            )
            .help(format!(
                "take one of these out of `{}`, so the steps can be ordered",
                names::AFTER
            ));
            for (node, edge) in &found[1..] {
                let step = steps[*node].0.as_str();
                let at = locate(declaration, file, &[names::STEPS, step, names::AFTER]);
                report = report.related(
                    at.location(),
                    format!("`{step}` runs after `{}` here", steps[edge.to].0),
                );
            }
            let (first, edge) = found[0];
            self.reference(
                declaration,
                file,
                &[names::STEPS, steps[first].0.as_str(), names::AFTER],
                edge.at,
                report,
            );
        }
        self.project = None;
    }

    // who claims each name in a shared name space, among the projects the checker accepted, by the
    // document's one definition of a claim.
    fn claims<'c>(
        &self,
        checked: &'c [Checked<'a>],
        shared: Shared,
    ) -> BTreeMap<&'c Identifier, Vec<&'c Identifier>> {
        let accepted = checked.iter().filter_map(|project| {
            let (_, declared) = project.accepted.as_ref()?;
            Some((&project.name, declared))
        });
        claims(accepted, move |declared| shared.names(declared))
    }

    // each name in a shared name space has one owner. the first project in name order keeps it,
    // and each other one is reported against its own declaration, pointing at the first.
    fn duplicates(
        &mut self,
        checked: &[Checked<'a>],
        index: &HashMap<&str, usize>,
        claims: &BTreeMap<&Identifier, Vec<&Identifier>>,
        shared: Shared,
    ) {
        let kind = shared.field();
        let located = |owner: &Identifier| {
            let project = &checked[index[owner.as_str()]];
            let (declaration, _) = project
                .accepted
                .as_ref()
                .expect("a claim is an accepted project's");
            (project, *declaration)
        };
        for (entry, owners) in claims {
            let Some((first, others)) = owners.split_first() else {
                continue;
            };
            let entry = entry.as_str();
            let (first_project, first_declaration) = located(first);
            let first_here = locate(
                first_declaration,
                Here::File(&first_project.file),
                &[kind, entry],
            )
            .location();
            for other in others {
                let (other_project, declaration) = located(other);
                self.project = Some(other_project.name.clone());
                self.path = vec![
                    Segment::Field(kind.to_owned()),
                    Segment::Field(entry.to_owned()),
                ];
                let here = locate(declaration, Here::File(&other_project.file), &[kind, entry]);
                self.report(
                    here,
                    finding(Code::Duplicate, format!("is already declared by `{first}`"))
                        .help(shared.help(entry, first))
                        .related(first_here.clone(), format!("`{first}` declares it here")),
                );
            }
        }
        self.project = None;
        self.path.clear();
    }

    fn manifest_entry(&self, name: &str) -> Here<'a> {
        locate(
            &self.manifest.value,
            Here::File(&self.manifest.file),
            &[ManifestField::Projects.name(), name],
        )
    }

    fn unknown_option<'k>(
        &mut self,
        here: Here<'_>,
        name: &str,
        known: impl IntoIterator<Item = &'k str>,
    ) {
        let mut found = finding(Code::UnknownOption, "is not an option here");
        if let Some(closest) = closest(name, known) {
            found = found.help(format!("did you mean `{closest}`?"));
        }
        self.report(here, found);
    }

    fn attribute_set<'h>(
        &mut self,
        node: &'h Node,
        here: Here<'_>,
        expected: &str,
    ) -> Option<&'h BTreeMap<String, Field>> {
        match node {
            Node::Attrs { fields } => Some(fields),
            other => {
                self.wrong_kind(here, expected, other);
                None
            }
        }
    }

    fn wrong_kind(&mut self, here: Here<'_>, expected: &str, node: &Node) {
        let found = match node {
            Node::TooLarge { values, depth } => finding(
                Code::Invalid,
                format!(
                    "holds more than {values} values or nests deeper than {depth} levels, as a value that refers to itself does"
                ),
            ),
            other => finding(
                Code::Type,
                format!("must be {expected}, not {}", other.kind()),
            ),
        };
        self.report(here, found);
    }

    // an attribute name the path may carry: one that would put the repository's location or a
    // store path into a finding is reported against its parent instead, and its value isn't read.
    fn safe_key(&mut self, name: &str, here: Here<'_>) -> bool {
        match self.text_problem(name, false) {
            None => true,
            Some(problem) => {
                let segment = self.path.pop();
                let owner = if self.path.is_empty() {
                    "a top-level attribute has a name that"
                } else {
                    "has a name that"
                };
                self.report(here, finding(problem.0, format!("{owner} {}", problem.1)));
                if let Some(segment) = segment {
                    self.path.push(segment);
                }
                false
            }
        }
    }

    // a name an attribute set keyed by identifiers may have.
    fn key(&mut self, name: &str, here: Here<'_>) -> bool {
        self.keyed(Key::Identifier, name, here)
    }

    // a name an attribute set keyed by `key` may have.
    fn keyed(&mut self, key: Key, name: &str, here: Here<'_>) -> bool {
        if !self.safe_key(name, here) {
            return false;
        }
        if !key.valid(name) {
            self.report(
                here,
                finding(Code::Invalid, format!("is not {}", key.accepts())),
            );
            return false;
        }
        if let Some(why) = key.reserved(name) {
            self.report(here, finding(Code::Invalid, why));
            return false;
        }
        true
    }

    // why text can't reach the document, if it can't. the location is tested before the store
    // pattern: a checkout that is itself a store path, as a fixture in a build is, would otherwise
    // report its own location as a store reference.
    fn text_problem(&self, value: &str, context: bool) -> Option<(Code, &'static str)> {
        if context {
            Some((
                Code::StoreReference,
                "refers to a store path, and the registry document carries none",
            ))
        } else if self.root != "/" && normalized(value).contains(self.root) {
            Some((
                Code::Location,
                "contains the repository's location, which differs between worktrees",
            ))
        } else if contains_store_path(value) {
            Some((
                Code::StoreReference,
                "refers to a store path, and the registry document carries none",
            ))
        } else {
            None
        }
    }

    fn string<'h>(&mut self, node: &'h Node, here: Here<'_>, expected: &str) -> Option<&'h str> {
        match node {
            Node::String { value, context } => match self.text_problem(value, *context) {
                None => Some(value.as_str()),
                Some((code, message)) => {
                    self.report(here, finding(code, message));
                    None
                }
            },
            other => {
                self.wrong_kind(here, expected, other);
                None
            }
        }
    }

    fn scalar<'h>(&mut self, scalar: Scalar, node: &'h Node, here: Here<'_>) -> Option<&'h str> {
        let value = self.string(node, here, scalar.expected())?;
        if scalar.valid(value) {
            Some(value)
        } else {
            self.report(
                here,
                finding(Code::Invalid, format!("must be {}", scalar.accepts())),
            );
            None
        }
    }

    // checks one value against its type and returns it as the document would carry it, or None
    // when anything in it is wrong; every finding is collected on the way.
    fn value<'h>(&mut self, ty: &Type, node: &'h Node, here: Here<'h>) -> Option<Value> {
        match ty {
            Type::Scalar(scalar) => self
                .scalar(*scalar, node, here)
                .map(|value| Value::String(value.to_owned())),
            Type::Enum(values) => {
                let value = self.string(node, here, "a string")?;
                if values.iter().any(|allowed| allowed == value) {
                    Some(Value::String(value.to_owned()))
                } else {
                    self.report(
                        here,
                        finding(Code::Enum, format!("must be {}", ty.describe())),
                    );
                    None
                }
            }
            // text that says something, and nothing more. it names no store path, since a command
            // runs what its shell's packages put on the path, and those come from the catalog.
            Type::Command => match node {
                Node::String { value, context } if *context || contains_store_path(value) => {
                    self.report(
                        here,
                        finding(
                            Code::StoreReference,
                            "names a store path, so it runs a package the catalog doesn't hold",
                        )
                        .help(format!(
                            "pick the package under `{}` and run it by name: a command runs what its shell's path holds",
                            names::PACKAGES
                        )),
                    );
                    None
                }
                Node::String { value, .. } if Scalar::Text.valid(value) => Some(Value::Null),
                Node::String { .. } => {
                    self.report(
                        here,
                        finding(Code::Invalid, format!("must be {}", Scalar::Text.accepts())),
                    );
                    None
                }
                other => {
                    self.wrong_kind(here, "a command line string", other);
                    None
                }
            },
            Type::Json => self.json(node, here),
            Type::List { element, non_empty } => {
                let Node::List { items } = node else {
                    self.wrong_kind(here, "a list", node);
                    return None;
                };
                let values = self.items(items, |this, item| this.value(element, item, here));
                if *non_empty && items.is_empty() {
                    self.report(here, finding(Code::Invalid, "must not be empty"));
                    return None;
                }
                values.map(Value::Array)
            }
            Type::Attrs { key, element } => {
                let fields = self.attribute_set(node, here, "an attribute set")?;
                self.entries(
                    fields,
                    here,
                    |this, name, here| this.keyed(*key, name, here),
                    |this, field, here| this.value(element, &field.value, here),
                )
                .map(Value::Object)
            }
            Type::Record(record) => self.record(record, node, here),
            Type::Function => match node {
                Node::Function {} => Some(Value::Null),
                other => {
                    self.wrong_kind(here, "a function, `final: prev: ...`", other);
                    None
                }
            },
            Type::AttributePath => {
                let path = self.string(node, here, "an attribute path string")?;
                if is_attribute_path(path) {
                    Some(Value::String(path.to_owned()))
                } else {
                    self.report(
                        here,
                        finding(Code::Invalid, format!("must be {ATTRIBUTE_PATH_ACCEPTS}")),
                    );
                    None
                }
            }
            // text, empty included, since an empty variable is a value; or where in a catalog
            // package the variable points.
            Type::EnvValue(location) => match node {
                Node::String { value, context } => match self.text_problem(value, *context) {
                    None => Some(Value::String(value.clone())),
                    Some((code, message)) => {
                        self.report(here, finding(code, message));
                        None
                    }
                },
                Node::Attrs { .. } => self.record(location, node, here),
                other => {
                    self.wrong_kind(here, &ty.describe(), other);
                    None
                }
            },
            Type::SecondOverlay => {
                let mut report = finding(
                    Code::SecondOverlay,
                    format!(
                        "is a second overlay: the repository's one overlay is the catalog's `{}`",
                        names::OVERRIDES
                    ),
                )
                .help(format!(
                    "move each entry to the catalog's `{}`, with a `{}` and a `{}`",
                    names::OVERRIDES,
                    names::REASON,
                    names::RELEASE
                ));
                if let Some(overlay) = &self.overlay {
                    report = report.related(overlay.clone(), "the one overlay is here");
                }
                self.report(here, report);
                None
            }
            Type::Schema => self.schema(node, here).map(|_| Value::Null),
        }
    }

    fn record<'h>(&mut self, record: &Record, node: &'h Node, here: Here<'h>) -> Option<Value> {
        let fields = self.attribute_set(node, here, "an attribute set")?;
        let mut values = Some(Map::new());
        for (name, field) in fields {
            let field_here = here.within(field);
            self.path.push(Segment::Field(name.clone()));
            let checked = if !self.safe_key(name, field_here) {
                None
            } else if let Some(declared) = record.get(name) {
                self.value(&declared.ty, &field.value, field_here)
                    .map(|value| declared.ty.carried().then(|| (name.clone(), value)))
            } else {
                self.unknown_option(field_here, name, record.names());
                None
            };
            self.path.pop();
            match checked {
                None => values = None,
                Some(None) => {}
                Some(carried) => keep(&mut values, carried),
            }
        }
        if self.missing(record, fields, here) {
            values = None;
        }
        for (name, declared) in record.fields() {
            if let Presence::Default(default) = &declared.presence
                && !fields.contains_key(name)
            {
                keep(&mut values, Some((name.clone(), default.json())));
            }
        }
        values.map(Value::Object)
    }

    // JSON data: what Nix can write as JSON without a store path, a function or a path.
    fn json<'h>(&mut self, node: &'h Node, here: Here<'h>) -> Option<Value> {
        const EXPECTED: &str =
            "JSON data: a string, number, boolean, null, list or attribute set of them";
        match node {
            Node::String { .. } => self
                .string(node, here, EXPECTED)
                .map(|value| Value::String(value.to_owned())),
            Node::Int { value } => Some(Value::from(*value)),
            Node::Float { value } => match serde_json::Number::from_f64(*value) {
                Some(number) => Some(Value::Number(number)),
                None => {
                    self.wrong_kind(here, EXPECTED, &Node::NonFinite {});
                    None
                }
            },
            Node::Bool { value } => Some(Value::Bool(*value)),
            Node::Null {} => Some(Value::Null),
            Node::List { items } => self
                .items(items, |this, item| this.json(item, here))
                .map(Value::Array),
            Node::Attrs { fields } => self
                .entries(fields, here, Self::safe_key, |this, field, here| {
                    this.json(&field.value, here)
                })
                .map(Value::Object),
            other => {
                self.wrong_kind(here, EXPECTED, other);
                None
            }
        }
    }

    // a list's elements, each checked by `each` under its index.
    fn items<'h>(
        &mut self,
        items: &'h [Node],
        mut each: impl FnMut(&mut Self, &'h Node) -> Option<Value>,
    ) -> Option<Vec<Value>> {
        let mut values = Some(Vec::with_capacity(items.len()));
        for (index, item) in items.iter().enumerate() {
            self.path.push(Segment::Index(index_segment(index)));
            let value = each(self, item);
            self.path.pop();
            keep(&mut values, value);
        }
        values
    }

    // an attribute set's fields, each name held to `name`'s rule and each value checked by `each`
    // under its name; a value whose name breaks the rule isn't read.
    fn entries<'h>(
        &mut self,
        fields: &'h BTreeMap<String, Field>,
        here: Here<'h>,
        mut name: impl FnMut(&mut Self, &str, Here<'_>) -> bool,
        mut each: impl FnMut(&mut Self, &'h Field, Here<'h>) -> Option<Value>,
    ) -> Option<Map<String, Value>> {
        let mut values = Some(Map::new());
        for (key, field) in fields {
            let here = here.within(field);
            self.path.push(Segment::Field(key.clone()));
            let value = if name(&mut *self, key, here) {
                each(self, field, here)
            } else {
                None
            };
            self.path.pop();
            keep(&mut values, value.map(|value| (key.clone(), value)));
        }
        values
    }
}

// the catalog as the document carries it, from its checked value and its manifest entry's. a
// value the document can't hold means the catalog's record and its type disagree, a defect in
// Shikumi that comes back as the same error a project's would.
fn accepted_catalog(value: Value, entry: &Value) -> Result<Catalog, CheckError> {
    let catalog = vocabulary::document_catalog_value(value, entry);
    serde_json::from_value(catalog).map_err(|error| CheckError {
        subject: "the catalog".to_owned(),
        error,
    })
}

// the sources of the composition's shell, as the document's one definition orders them, each
// with the declaration it's located in.
fn sources<'c, 'a>(
    checked: &'c [Checked<'a>],
    index: &HashMap<&str, usize>,
    focus: Option<&Identifier>,
) -> Vec<Source<'c, 'a>> {
    let accepted = checked.iter().filter_map(|project| {
        let (_, declared) = project.accepted.as_ref()?;
        Some((&project.name, declared))
    });
    shell(accepted, focus)
        .into_iter()
        .filter_map(|shell| {
            let project = &checked[*index.get(shell.owner.as_str())?];
            let (declaration, _) = project.accepted.as_ref()?;
            Some(Source {
                project,
                declaration,
                shell,
            })
        })
        .collect()
}

fn document(
    findings: Vec<Diagnostic>,
    focus: Option<Identifier>,
    directories: &Directories,
    checked: Vec<Checked<'_>>,
    catalog: Option<Catalog>,
) -> Document {
    let accepted = findings.is_empty();
    // with nothing found, every project was accepted. one that somehow wasn't leaves the document
    // without projects, which the shared refusal names as a broken contract.
    let projects = accepted
        .then(|| {
            checked
                .into_iter()
                .map(|project| {
                    let (_, accepted) = project.accepted?;
                    Some((project.name, accepted))
                })
                .collect::<Option<BTreeMap<_, _>>>()
        })
        .flatten();
    let remainder = (accepted && focus.is_none()).then(|| {
        let mut excludes: Vec<RelativePath> = directories.values().flatten().cloned().collect();
        excludes.sort();
        Remainder { excludes }
    });
    Document {
        schema: SchemaName,
        version: Version,
        focus,
        catalog: catalog.filter(|_| accepted),
        projects,
        remainder,
        diagnostics: findings,
    }
}

fn diagnostic(
    project: Option<Identifier>,
    path: Vec<Segment>,
    here: Here<'_>,
    finding: Finding,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: finding.code,
        project,
        path,
        location: here.location(),
        message: text(finding.message),
        help: finding.help.map(text),
        related: finding.related,
    }
}

// a failed evaluation of `file`, as the finding its kind calls for: where it goes, the field's path
// when it names one, and the finding. a failure of the user's sits at the innermost place in their
// own code the trace passed through, or at the file where the evaluator gives none there, with the
// rest of their places as notes. a bug in Shikumi is reported at the file as Shikumi's, with its
// frames for the report, and never shown as the user's.
fn failure<'f>(file: &'f RelativePath, failure: &'f Failure) -> (Here<'f>, Vec<Segment>, Finding) {
    let detail = failure.message.as_str();
    let origin = match &failure.origin {
        Origin::Shikumi(origin) => {
            let frames: Vec<String> = origin
                .frames
                .iter()
                .take(INTERNAL_FRAMES)
                .map(|frame| format!("{}:{}:{}", frame.file, frame.line, frame.column))
                .collect();
            let report = finding(
                Code::ShikumiBug,
                format!(
                    "evaluating this file failed inside Shikumi's own Nix, which is a bug in Shikumi, not in this file: {detail}"
                ),
            )
            .help(format!(
                "report it to Shikumi with this message and the frames it failed in, innermost first: {}",
                frames.join(", ")
            ));
            return (Here::File(file), Vec::new(), report);
        }
        Origin::User(origin) => origin,
    };
    let mut report = match failure.kind {
        FailureKind::Syntax => finding(Code::Syntax, format!("this file isn't valid Nix: {detail}")),
        FailureKind::UndefinedVariable => finding(
            Code::UndefinedVariable,
            format!("`{detail}` is used here, but nothing defines it"),
        )
        .help(format!(
            "define it in a `let`, or name it as an argument: a declaration file is called with {}, and the manifest with {}",
            DECLARATION.called_with(),
            MANIFEST.called_with()
        )),
        FailureKind::Type => finding(
            Code::TypeError,
            format!("evaluation stopped at a value of the wrong type: {detail}"),
        ),
        FailureKind::Thrown => finding(Code::Thrown, format!("evaluation threw an error: {detail}")),
        FailureKind::Assertion => finding(Code::Assertion, "this assertion failed"),
        FailureKind::InvalidUtf8 => finding(
            Code::InvalidUtf8,
            if origin.field.is_some() {
                "holds text that isn't valid UTF-8, which the registry document can't carry"
            } else {
                "this file's declaration holds text that isn't valid UTF-8, which the registry document can't carry"
            },
        )
        .help("cutting text by bytes, as `builtins.substring` does, can split a character in two"),
        FailureKind::Unrecognized => {
            finding(Code::Evaluation, format!("evaluation failed: {detail}"))
        }
        FailureKind::Crashed => finding(
            Code::Evaluation,
            format!("the evaluator stopped without reporting an error: {detail}"),
        ),
    };
    let here = origin
        .field
        .as_ref()
        .and_then(|field| field.at.as_ref())
        .or(origin.frames.first())
        .map_or(Here::File(file), Here::At);
    // where the user's code led to it, outermost last, each place once.
    let mut shown: Vec<&At> = origin.frames.iter().take(1).collect();
    for at in origin.frames.iter().skip(1) {
        if shown.len() > RELATED_FRAMES {
            break;
        }
        if !shown.contains(&at) {
            shown.push(at);
            report = report.related(at.location(), "reached from here");
        }
    }
    let path = origin
        .field
        .as_ref()
        .map(|field| field.path.clone())
        .unwrap_or_default();
    (here, path, report)
}

// adds a checked child to what's collected, or drops the collection once any child had a finding.
// every child is still checked after that, so one pass reports all of them.
fn keep<T, C: Extend<T>>(collected: &mut Option<C>, child: Option<T>) {
    match (collected.as_mut(), child) {
        (Some(all), Some(child)) => all.extend([child]),
        (_, None) => *collected = None,
        (None, Some(_)) => {}
    }
}

// the nearest position along a path in a serialized value, falling back to where the value was
// read from when nothing on the path has one.
fn locate<'h>(node: &'h Node, mut here: Here<'h>, path: &[&str]) -> Here<'h> {
    let mut node = node;
    for segment in path {
        let Node::Attrs { fields } = node else {
            break;
        };
        let Some(field) = fields.get(*segment) else {
            break;
        };
        here = here.within(field);
        node = &field.value;
    }
    here
}

// the value at a path of attribute names in a serialized value.
fn field_at<'n>(node: &'n Node, path: &[&str]) -> Option<&'n Node> {
    path.iter().try_fold(node, |node, name| match node {
        Node::Attrs { fields } => fields.get(*name).map(|field| &field.value),
        _ => None,
    })
}

// the nearest position along a finding's path, which follows attribute names and stops at a list
// element, since elements have no position of their own.
fn locate_path<'h>(node: &'h Node, here: Here<'h>, path: &[Segment]) -> Here<'h> {
    let names: Vec<&str> = path
        .iter()
        .map_while(|segment| match segment {
            Segment::Field(name) => Some(name.as_str()),
            Segment::Index(_) => None,
        })
        .collect();
    locate(node, here, &names)
}

fn index_segment(index: usize) -> u32 {
    u32::try_from(index).expect("a declaration's list has fewer than 2^32 elements")
}

// whether text holds a store path: a path component that starts with a store hash, 32 characters
// of Nix's base-32 alphabet, ending there or followed by a dash. matching the component rather
// than the store directory catches every spelling of the directory, such as `/nix//store` or
// `/nix/var/../store`, and a store path written without its name. matching text at all catches a
// path whose context was stripped, by `match` or `substring` as much as by
// `unsafeDiscardStringContext`.
fn contains_store_path(text: &str) -> bool {
    const HASH: usize = 32;
    let base32 = |b: &u8| b.is_ascii_digit() || (b.is_ascii_lowercase() && !b"eotu".contains(b));
    text.split('/').skip(1).any(|component| {
        let bytes = component.as_bytes();
        bytes.len() >= HASH
            && bytes[..HASH].iter().all(base32)
            && bytes.get(HASH).is_none_or(|&b| b == b'-')
    })
}

// text with doubled slashes and `.` components taken out, so a path the evaluator would read as
// the repository's location is compared as that location. it allocates only for text that has
// either.
fn normalized(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains("//") && !text.contains("/./") && !text.ends_with("/.") {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('/') {
        out.push_str(&rest[..index]);
        rest = rest[index..].trim_start_matches('/');
        while let Some(after) = rest.strip_prefix("./").or((rest == ".").then_some("")) {
            rest = after.trim_start_matches('/');
        }
        out.push('/');
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

// the known name closest to a mistyped one, when it's close enough to be the likely intent.
fn closest<'k>(name: &str, known: impl IntoIterator<Item = &'k str>) -> Option<&'k str> {
    known
        .into_iter()
        .map(|candidate| (distance(name, candidate), candidate))
        .filter(|(distance, candidate)| *distance <= (candidate.len() / 3).max(1))
        .min()
        .map(|(_, candidate)| candidate)
}

// Levenshtein distance over characters, in one row. names are a handful of characters, so the
// quadratic cost is a few hundred steps at most.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous + usize::from(ca != *cb);
            previous = row[j + 1];
            row[j + 1] = substitution.min(row[j] + 1).min(previous + 1);
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::contract::render_json;
    use crate::document::ShowPath;

    // the tracked fixtures hold the repository root as `@root@`, since every checkout puts it
    // somewhere else; the checker sees it back where a string held it.
    const ROOT: &str = "/repo";

    fn checked(declarations: &Declarations, root: &str) -> Document {
        check(declarations, root).unwrap()
    }

    fn fixture(tracked: &str) -> Declarations {
        Declarations::from_json(tracked.replace("@root@", ROOT).as_bytes()).unwrap()
    }

    fn broken() -> Document {
        checked(
            &fixture(include_str!("../fixtures/broken/declarations.json")),
            ROOT,
        )
    }

    // a finding as a test names it: its code, its project, its path as Nix selects it, and its
    // file and line.
    type Expected = (
        Code,
        Option<&'static str>,
        &'static str,
        &'static str,
        Option<u32>,
    );

    fn shown(diagnostic: &Diagnostic) -> (Code, Option<String>, String, String, Option<u32>) {
        (
            diagnostic.code,
            diagnostic.project.as_ref().map(ToString::to_string),
            ShowPath(&diagnostic.path).to_string(),
            diagnostic.location.file.to_string(),
            diagnostic.location.position.map(|p| p.line.get()),
        )
    }

    fn expected(row: &Expected) -> (Code, Option<String>, String, String, Option<u32>) {
        (
            row.0,
            row.1.map(str::to_owned),
            row.2.to_owned(),
            row.3.to_owned(),
            row.4,
        )
    }

    // each refusal is written once: it gets a test named for the rule it holds, so removing the
    // rule fails that test, and a place in the one-pass test, in the order findings come back.
    macro_rules! refusals {
        ($($test:ident: $code:ident, $project:expr, $path:expr, $file:expr, $line:expr;)*) => {
            $(
                #[test]
                fn $test() {
                    let row: Expected = (Code::$code, $project, $path, $file, $line);
                    let found: Vec<_> = broken().diagnostics.iter().map(shown).collect();
                    assert!(
                        found.contains(&expected(&row)),
                        "{row:?} isn't among the findings:\n{found:#?}"
                    );
                }
            )*
            const EVERY: &[Expected] = &[$((Code::$code, $project, $path, $file, $line)),*];
        };
    }

    refusals! {
        refuses_an_extension_schema_outside_the_vocabulary:
            Invalid, None, "extensions.broken", "shikumi.nix", Some(37);
        refuses_an_extension_schema_built_from_an_internal_type:
            Invalid, None, "extensions.sneaky", "shikumi.nix", Some(41);
        refuses_an_unknown_manifest_option:
            UnknownOption, None, "owners", "shikumi.nix", Some(45);
        refuses_a_directory_outside_the_repository:
            Invalid, Some("outside"), "projects.outside", "shikumi.nix", Some(19);
        refuses_a_directory_inside_another_project:
            Overlap, Some("overlap"), "projects.overlap", "shikumi.nix", Some(15);
        refuses_a_system_nixpkgs_does_not_know:
            Invalid, None, "catalog.systems[1]", "shikumi.nix", Some(48);
        refuses_an_overlay_beside_the_catalogs_overrides:
            SecondOverlay, None, "overlays", "catalog/catalog.nix", Some(24);
        refuses_an_override_without_what_lets_it_go:
            Missing, None, "overrides.cowsay.release", "catalog/catalog.nix", Some(19);
        refuses_an_override_without_why_it_exists:
            Missing, None, "overrides.hello.reason", "catalog/catalog.nix", Some(15);
        refuses_a_catalog_entry_that_names_no_package_in_nixpkgs:
            UnknownReference, None, "packages.missing", "catalog/catalog.nix", Some(7);
        refuses_a_listed_directory_without_a_declaration:
            NoDeclaration, Some("absent"), "projects.absent", "shikumi.nix", Some(18);
        refuses_an_invalid_identifier_in_a_list:
            Invalid, Some("bad-names"), "project.dependsOn[0]", "projects/bad-names/project.nix", Some(6);
        refuses_a_name_that_is_not_an_identifier:
            Invalid, Some("bad-names"), "steps.Build", "projects/bad-names/project.nix", Some(8);
        refuses_a_value_outside_a_closed_enumeration:
            Enum, Some("closed-enum"), "services.worker.restart", "projects/closed-enum/project.nix", Some(10);
        refuses_an_empty_required_list:
            Invalid, Some("empty-list"), "project.maintainers", "projects/empty-list/project.nix", Some(5);
        refuses_a_glob_that_leaves_the_repository:
            Invalid, Some("extensions"), "extensions.layout.pages[0]", "projects/extensions/project.nix", Some(12);
        refuses_a_path_in_json_data:
            Type, Some("extensions"), "extensions.meta.logo", "projects/extensions/project.nix", Some(9);
        refuses_an_extension_namespace_without_a_schema:
            UnknownOption, Some("extensions"), "extensions.nobody", "projects/extensions/project.nix", Some(10);
        refuses_an_extension_namespace_whose_schema_was_refused:
            UnknownOption, Some("extensions"), "extensions.sneaky", "projects/extensions/project.nix", Some(11);
        refuses_an_extension_value_its_schema_rejects:
            Invalid, Some("extensions"), "extensions.team.owner", "projects/extensions/project.nix", Some(8);
        refuses_a_store_path_stripped_of_its_context:
            StoreReference, Some("laundered"), "project.summary", "projects/laundered/project.nix", Some(8);
        refuses_the_repository_location_as_a_name:
            Location, Some("location"), "extensions.meta", "projects/location/project.nix", Some(7);
        refuses_the_repository_location:
            Location, Some("location"), "project.summary", "projects/location/project.nix", Some(4);
        refuses_a_missing_required_field:
            Missing, Some("missing"), "project.summary", "projects/missing/project.nix", Some(3);
        refuses_a_missing_required_field_of_a_named_record:
            Missing, Some("missing"), "steps.build.exec", "projects/missing/project.nix", Some(4);
        refuses_a_record_that_skipped_the_factory:
            NotADeclaration, Some("no-factory"), "", "projects/no-factory/project.nix", None;
        refuses_an_overlay_a_project_declares:
            SecondOverlay, Some("overlay"), "overrides", "projects/overlay/project.nix", Some(7);
        refuses_a_variable_the_shell_sets_itself:
            Invalid, Some("path-variable"), "env.PATH", "projects/path-variable/project.nix", Some(8);
        refuses_a_field_whose_attributes_have_no_position_at_the_nearest_one_that_has:
            Missing, Some("split"), "commands.check.exec", "projects/split/project.nix", Some(8);
        refuses_a_value_from_an_imported_file_in_that_file:
            Invalid, Some("split"), "steps.build.after[0]", "projects/split/steps.nix", Some(6);
        refuses_a_store_reference:
            StoreReference, Some("store-reference"), "project.summary", "projects/store-reference/project.nix", Some(11);
        refuses_an_unknown_field:
            UnknownOption, Some("unknown-options"), "project.owner", "projects/unknown-options/project.nix", Some(6);
        refuses_an_unknown_field_of_a_named_record:
            UnknownOption, Some("unknown-options"), "steps.lint.cmd", "projects/unknown-options/project.nix", Some(12);
        refuses_an_unknown_top_level_field:
            UnknownOption, Some("unknown-options"), "tasks", "projects/unknown-options/project.nix", Some(8);
        refuses_a_function_where_a_command_line_belongs:
            Type, Some("wrong-type"), "commands.run.exec", "projects/wrong-type/project.nix", Some(10);
        refuses_a_value_of_the_wrong_type:
            Type, Some("wrong-type"), "project.maintainers", "projects/wrong-type/project.nix", Some(5);
        refuses_a_list_where_an_attribute_set_belongs:
            Type, Some("wrong-type"), "steps", "projects/wrong-type/project.nix", Some(7);
        refuses_a_dependency_on_an_unlisted_project:
            UnknownReference, Some("dangling"), "project.dependsOn[1]", "projects/dangling/project.nix", Some(6);
        refuses_ordering_after_a_missing_step:
            UnknownReference, Some("dangling"), "steps.test.after[0]", "projects/dangling/project.nix", Some(14);
        refuses_an_import_of_a_toolchain_its_owner_does_not_define:
            UnknownReference, Some("jvm-app"), "project.imports.rust", "projects/jvm-app/project.nix", Some(8);
        refuses_a_package_outside_the_catalog:
            UnknownReference, Some("picks"), "packages[1]", "projects/picks/project.nix", Some(7);
        refuses_a_variable_in_a_package_outside_the_catalog:
            UnknownReference, Some("picks"), "env.TOOL_HOME.package", "projects/picks/project.nix", Some(11);
        refuses_projects_that_depend_on_each_other_in_a_loop:
            Cycle, Some("cycle-a"), "project.dependsOn[1]", "projects/cycle-a/project.nix", Some(6);
        refuses_steps_that_run_after_each_other_in_a_loop:
            Cycle, Some("cycle-steps"), "steps.build.after[0]", "projects/cycle-steps/project.nix", Some(11);
        refuses_a_command_two_projects_declare:
            Duplicate, Some("duplicate-b"), "commands.fmt", "projects/duplicate-b/project.nix", Some(7);
        refuses_a_service_two_projects_declare:
            Duplicate, Some("duplicate-b"), "services.db", "projects/duplicate-b/project.nix", Some(11);
        refuses_a_toolchain_two_projects_define:
            Duplicate, Some("jdk-owner"), "toolchains.jdk", "projects/jdk-owner/project.nix", Some(7);
        refuses_a_variable_a_shell_gets_from_a_project_and_a_toolchain:
            Duplicate, Some("jvm-app"), "env.JAVA_HOME", "projects/jvm-app/project.nix", Some(12);
        refuses_two_jdks_on_one_path_one_of_them_propagated:
            Conflict, Some("jvm-app"), "packages[0]", "projects/jvm-app/project.nix", Some(11);
    }

    // what one finding says, as a person reads it, and every file it names.
    fn named_in(diagnostic: &Diagnostic) -> (String, Vec<String>) {
        let mut files = vec![diagnostic.location.file.to_string()];
        files.extend(
            diagnostic
                .related
                .iter()
                .map(|r| r.location.file.to_string()),
        );
        (diagnostic.message.to_string(), files)
    }

    fn the(document: &Document, code: Code, path: &str) -> Diagnostic {
        document
            .diagnostics
            .iter()
            .find(|d| d.code == code && ShowPath(&d.path).to_string() == path)
            .unwrap_or_else(|| panic!("no {code} at {path}: {:#?}", document.diagnostics))
            .clone()
    }

    #[test]
    fn a_conflict_names_both_packages_and_both_files() {
        let (message, files) = named_in(&the(&broken(), Code::Conflict, "packages[0]"));
        for named in [
            "`openjdk-25.",
            "`openjdk-21.",
            "`kotlin`",
            "`jdk`",
            "`openjdk`",
            "x86_64-linux",
        ] {
            assert!(message.contains(named), "{named} isn't in {message}");
        }
        assert_eq!(
            files,
            [
                "projects/jvm-app/project.nix",
                "projects/jdk-owner/project.nix"
            ]
        );
    }

    #[test]
    fn a_second_definition_names_both_files() {
        for (code, path, files) in [
            (
                Code::Duplicate,
                "env.JAVA_HOME",
                [
                    "projects/jvm-app/project.nix",
                    "projects/jdk-owner/project.nix",
                ],
            ),
            (
                Code::Duplicate,
                "toolchains.jdk",
                [
                    "projects/jdk-owner/project.nix",
                    "projects/jdk-again/project.nix",
                ],
            ),
            (
                Code::SecondOverlay,
                "overrides",
                ["projects/overlay/project.nix", "catalog/catalog.nix"],
            ),
        ] {
            assert_eq!(named_in(&the(&broken(), code, path)).1, files, "{path}");
        }
    }

    // a focused composition's shell holds its project and the toolchains it uses, so what's
    // refused on that path is refused there too, and what lies on other projects' paths isn't.
    #[test]
    fn a_focused_shell_holds_its_project_and_the_toolchains_it_uses() {
        let mut declarations = fixture(include_str!("../fixtures/broken/declarations.json"));
        let codes = |declarations: &Declarations, path: &str| -> Vec<Code> {
            checked(declarations, ROOT)
                .diagnostics
                .iter()
                .filter(|d| {
                    d.project.as_ref().is_some_and(|p| p.as_str() == "jvm-app")
                        && ShowPath(&d.path).to_string() == path
                })
                .map(|d| d.code)
                .collect()
        };
        declarations.focus = Some("jvm-app".to_owned());
        assert_eq!(codes(&declarations, "packages[0]"), [Code::Conflict]);
        assert_eq!(codes(&declarations, "env.JAVA_HOME"), [Code::Duplicate]);
        // `picks` holds JDK 21 too, but not on `jdk-owner`'s path, which has no Kotlin.
        declarations.focus = Some("jdk-owner".to_owned());
        assert!(
            !checked(&declarations, ROOT)
                .diagnostics
                .iter()
                .any(|d| d.code == Code::Conflict)
        );
    }

    #[test]
    fn a_pick_in_a_repository_without_a_catalog_is_refused() {
        let mut declarations = fixture(include_str!("../fixtures/broken/declarations.json"));
        declarations.catalog = None;
        let Node::Attrs { fields } = manifest_value(&mut declarations) else {
            panic!()
        };
        fields.remove("catalog");
        let found = the(
            &checked(&declarations, ROOT),
            Code::UnknownReference,
            "packages[1]",
        );
        assert!(
            found
                .message
                .as_str()
                .contains("the repository has no catalog")
        );
        assert!(found.help.unwrap().as_str().contains("catalog = {"));
    }

    // a catalog that couldn't be read says nothing of what it holds, so no pick is checked
    // against it, and its own failure stands alone.
    #[test]
    fn a_catalog_that_failed_checks_no_pick_against_it() {
        let document = failures();
        let catalog: Vec<&Diagnostic> = document
            .diagnostics
            .iter()
            .filter(|d| d.location.file.as_str() == "catalog/catalog.nix")
            .collect();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].code, Code::Thrown);
        assert!(
            !document
                .diagnostics
                .iter()
                .any(|d| d.project.as_ref().is_some_and(|p| p.as_str() == "sound"))
        );
    }

    #[test]
    fn the_accepted_document_carries_the_catalog_toolchains_imports_and_variables() {
        let basic = checked(
            &fixture(include_str!("../fixtures/basic/declarations.json")),
            ROOT,
        );
        let catalog = basic.catalog.as_ref().unwrap();
        let packages: Vec<(&str, &str)> = catalog
            .packages
            .iter()
            .map(|(name, path)| (name.as_str(), path.as_str()))
            .collect();
        assert_eq!(
            packages,
            [
                ("jdk", "jdk21"),
                ("kotlin", "kotlin"),
                ("mdbook", "mdbook"),
                ("nodejs", "nodejs")
            ]
        );
        assert!(
            catalog
                .overrides
                .keys()
                .any(|name| name.as_str() == "kotlin")
        );
        let projects = basic.projects.as_ref().unwrap();
        let web = &projects[&Identifier::new("web").unwrap()];
        assert_eq!(web.imports.values().next().unwrap().as_str(), "core");
        let core = &projects[&Identifier::new("core").unwrap()];
        assert!(
            core.toolchains[&Identifier::new("jdk").unwrap()]
                .env
                .values()
                .all(|value| value.package().is_some())
        );
    }

    // the record and the document's type disagreeing is Shikumi's defect, reported as an error the
    // caller can show rather than a panic, for the catalog as for a project.
    #[test]
    fn a_catalog_the_document_cant_hold_is_an_error_not_a_panic() {
        let entry = json!({"directory": "catalog", "systems": ["x86_64-linux"]});
        let error = accepted_catalog(json!({"packages": 3, "overrides": {}}), &entry).unwrap_err();
        assert!(
            error.to_string().contains("the catalog in a shape"),
            "{error}"
        );
        let held = accepted_catalog(json!({"packages": {}, "overrides": {}}), &entry).unwrap();
        assert_eq!(held.packages.len(), 0);
    }

    #[test]
    fn every_finding_comes_back_in_one_pass() {
        let found: Vec<_> = broken().diagnostics.iter().map(shown).collect();
        let wanted: Vec<_> = EVERY.iter().map(expected).collect();
        assert_eq!(found, wanted);
    }

    // the tracked document is what `shikumi registry check --format json` prints for the broken
    // fixture on every Nix implementation, which the `registry-no-builds-*` checks hold.
    #[test]
    fn the_broken_fixture_is_refused_with_the_tracked_document() {
        let document = broken();
        assert_eq!(document.projects, None);
        assert_eq!(document.remainder, None);
        assert_eq!(
            render_json(&document),
            include_str!("../fixtures/broken/document.json")
        );
    }

    // the tracked rendering is what `shikumi registry check` prints for the broken fixture. every
    // implementation serializes the same declarations, which the `registry-no-builds-*` checks
    // hold, so this pins what each of them shows a person.
    #[test]
    fn the_broken_fixture_renders_as_the_tracked_findings() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../nix/declarations/tests/fixtures/broken");
        let rendered = crate::render::render(
            broken().diagnostics(),
            &mut crate::render::Sources::new(&fixture),
        );
        assert!(
            rendered == include_str!("../fixtures/broken/findings.txt"),
            "crates/registry/fixtures/broken/findings.txt differs from the rendering:\n{rendered}"
        );
    }

    fn failures() -> Document {
        checked(
            &fixture(include_str!("../fixtures/failures/declarations.json")),
            ROOT,
        )
    }

    // the failures fixture is what `shikumi registry declarations` serializes on CppNix 2.35; the
    // `registry-no-builds-*` checks hold every implementation's classification of it to the same
    // reference, and `registry-cli` holds the CLI to this document and rendering.
    #[test]
    fn each_failed_evaluation_is_its_projects_finding_and_the_others_are_still_checked() {
        assert_eq!(
            render_json(&failures()),
            include_str!("../fixtures/failures/document.json")
        );
    }

    #[test]
    fn the_failures_fixture_renders_at_the_users_lines() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../nix/declarations/tests/fixtures/failures");
        let rendered = crate::render::render(
            failures().diagnostics(),
            &mut crate::render::Sources::new(&fixture),
        );
        assert!(
            rendered == include_str!("../fixtures/failures/findings.txt"),
            "crates/registry/fixtures/failures/findings.txt differs from the rendering:\n{rendered}"
        );
    }

    // a failure of the user's with these frames, or, given frames of Shikumi's, a Shikumi bug.
    fn failure(kind: FailureKind, frames: Value, internal: Value) -> Value {
        let origin = if internal == json!([]) {
            json!({"user": {"frames": frames, "field": null}})
        } else {
            json!({"shikumi": {"frames": internal}})
        };
        json!({"failure": {"kind": kind.as_str(), "message": "boom", "origin": origin}})
    }

    fn located(file: &str, line: u32) -> Value {
        json!({"file": file, "line": line, "column": 3})
    }

    #[test]
    fn a_failure_only_inside_shikumis_nix_is_reported_as_shikumis_bug_at_the_projects_file() {
        let mut declarations = repository(node(sound()), None);
        declarations.projects.insert(
            "app".to_owned(),
            Some(
                serde_json::from_value(failure(
                    FailureKind::Type,
                    json!([]),
                    json!([located("serialize.nix", 128)]),
                ))
                .unwrap(),
            ),
        );
        let document = checked(&declarations, ROOT);
        let [finding] = document.diagnostics.as_slice() else {
            panic!("{:?}", document.diagnostics);
        };
        assert_eq!(finding.code, Code::ShikumiBug);
        assert_eq!(finding.location.to_string(), "app/project.nix");
        assert!(finding.message.as_str().contains("a bug in Shikumi"));
        assert!(
            finding
                .help
                .as_ref()
                .unwrap()
                .as_str()
                .ends_with("serialize.nix:128:3"),
            "{finding:?}"
        );
    }

    #[test]
    fn a_users_failure_shows_their_innermost_frame_first_and_the_rest_once_each() {
        let mut declarations = repository(node(sound()), None);
        let frames: Vec<Value> = [5, 4, 5, 3, 2, 1, 6, 7, 8, 9, 10, 11]
            .into_iter()
            .map(|line| located("app/helpers.nix", line))
            .collect();
        declarations.projects.insert(
            "app".to_owned(),
            Some(
                serde_json::from_value(failure(FailureKind::Thrown, json!(frames), json!([])))
                    .unwrap(),
            ),
        );
        let document = checked(&declarations, ROOT);
        let [finding] = document.diagnostics.as_slice() else {
            panic!("{:?}", document.diagnostics);
        };
        assert_eq!(finding.code, Code::Thrown);
        assert_eq!(finding.location.to_string(), "app/helpers.nix:5:3");
        let related: Vec<String> = finding
            .related
            .iter()
            .map(|related| related.location.to_string())
            .collect();
        let lines: Vec<u32> = vec![4, 3, 2, 1, 6, 7, 8, 9];
        let wanted: Vec<String> = lines
            .iter()
            .map(|line| format!("app/helpers.nix:{line}:3"))
            .collect();
        assert_eq!(related, wanted);
    }

    // every project's directory is the manifest's to say, so when the manifest fails nothing else
    // is read, and the failure is the one finding.
    #[test]
    fn a_manifest_that_fails_to_evaluate_is_the_one_finding() {
        let mut declarations = repository(node(sound()), None);
        declarations.manifest = serde_json::from_value(failure(
            FailureKind::Syntax,
            json!([located("shikumi.nix", 2)]),
            json!([]),
        ))
        .unwrap();
        declarations.projects.clear();
        let document = checked(&declarations, ROOT);
        let found: Vec<_> = document.diagnostics.iter().map(shown).collect();
        assert_eq!(
            found,
            [(
                Code::Syntax,
                None,
                String::new(),
                "shikumi.nix".to_owned(),
                Some(2)
            )]
        );
        assert_eq!(document.projects, None);
    }

    #[test]
    fn the_basic_fixture_is_accepted_as_the_tracked_document() {
        let document = checked(
            &fixture(include_str!("../fixtures/basic/declarations.json")),
            ROOT,
        );
        assert_eq!(document.diagnostics, []);
        assert_eq!(
            render_json(&document),
            include_str!("../fixtures/basic/document.json")
        );
    }

    #[test]
    fn a_duplicate_points_at_the_first_declaration() {
        let document = broken();
        let duplicate = document
            .diagnostics
            .iter()
            .find(|d| d.code == Code::Duplicate)
            .unwrap();
        assert_eq!(duplicate.related.len(), 1);
        assert_eq!(
            duplicate.related[0].location.to_string(),
            "projects/duplicate-a/project.nix:7:3"
        );
    }

    // a plain JSON value as the layer serializes it, every attribute without a position.
    fn node(value: Value) -> Value {
        match value {
            Value::String(value) => json!({"kind": "string", "value": value, "context": false}),
            Value::Bool(value) => json!({"kind": "bool", "value": value}),
            Value::Null => json!({"kind": "null"}),
            Value::Number(n) if n.is_i64() => json!({"kind": "int", "value": n}),
            Value::Number(n) => json!({"kind": "float", "value": n}),
            Value::Array(items) => {
                json!({"kind": "list", "items": items.into_iter().map(node).collect::<Vec<_>>()})
            }
            Value::Object(fields) => json!({
                "kind": "attrs",
                "fields": fields
                    .into_iter()
                    .map(|(name, value)| (name, json!({"at": null, "value": node(value)})))
                    .collect::<Map<_, _>>()
            }),
        }
    }

    // a repository of one project, `app` in `app/`, declaring `declaration` through the factory.
    fn repository(declaration: Value, focus: Option<&str>) -> Declarations {
        serde_json::from_value(json!({
            "schema": "shikumi.declarations",
            "version": {"major": 1, "minor": 0},
            "focus": focus,
            "manifest": {"file": "shikumi.nix", "value": node(json!({"projects": {"app": "app"}}))},
            "catalog": null,
            "projects": {"app": {"factory": true, "value": declaration}}
        }))
        .unwrap()
    }

    fn manifest_value(declarations: &mut Declarations) -> &mut Node {
        match &mut declarations.manifest {
            Evaluated::Value(manifest) => &mut manifest.value,
            Evaluated::Failed(_) => panic!("the test's manifest evaluated"),
        }
    }

    fn sound() -> Value {
        json!({"project": {"summary": "An app", "maintainers": ["someone"]}})
    }

    fn findings(declarations: &Declarations) -> Vec<(Code, String)> {
        checked(declarations, ROOT)
            .diagnostics
            .iter()
            .map(|d| (d.code, ShowPath(&d.path).to_string()))
            .collect()
    }

    fn with(field: &str, value: Value) -> Value {
        let mut declaration = node(sound());
        declaration["fields"]["project"]["value"]["fields"][field] =
            json!({"at": null, "value": value});
        declaration
    }

    #[test]
    fn a_value_too_large_to_read_is_refused_rather_than_read() {
        let declarations = repository(
            with(
                "summary",
                json!({"kind": "too-large", "values": 100000, "depth": 32}),
            ),
            None,
        );
        assert_eq!(
            findings(&declarations),
            [(Code::Invalid, "project.summary".to_owned())]
        );
    }

    #[test]
    fn a_number_that_is_not_finite_is_a_type_finding() {
        let declarations = repository(with("summary", json!({"kind": "non-finite"})), None);
        assert_eq!(
            findings(&declarations),
            [(Code::Type, "project.summary".to_owned())]
        );
    }

    #[test]
    fn a_derivation_where_text_belongs_is_a_type_finding() {
        let declarations = repository(with("summary", json!({"kind": "derivation"})), None);
        assert_eq!(
            findings(&declarations),
            [(Code::Type, "project.summary".to_owned())]
        );
    }

    #[test]
    fn a_string_with_context_is_a_store_reference_whatever_its_text() {
        let summary = json!({"kind": "string", "value": "plain", "context": true});
        assert_eq!(
            findings(&repository(with("summary", summary), None)),
            [(Code::StoreReference, "project.summary".to_owned())]
        );
    }

    #[test]
    fn a_store_path_is_matched_by_its_hash_in_any_spelling_of_the_store() {
        let hash = "0123456789abcdfghijklmnpqrsvwxyz";
        for text in [
            format!("at /nix/store/{hash}-x"),
            format!("/nix//store/{hash}-x/bin"),
            format!("/nix/store/./{hash}"),
            format!("/nix/var/../store/{hash}"),
            format!("/elsewhere/{hash}-name"),
        ] {
            assert!(contains_store_path(&text), "{text}");
        }
        assert!(!contains_store_path("the /nix/store directory"));
        // `e`, `o`, `t` and `u` aren't in Nix's base-32 alphabet.
        let not_a_hash = "e123456789abcdfghijklmnpqrsvwxyz";
        assert!(!contains_store_path(&format!("/nix/store/{not_a_hash}-x")));
        assert!(!contains_store_path(&format!("/nix/store/{hash}x")));
    }

    #[test]
    fn the_location_is_found_however_its_slashes_are_written() {
        let mut declaration = node(sound());
        declaration["fields"]["project"]["value"]["fields"]["summary"]["value"] =
            node(json!("at //repo//app"));
        declaration["fields"]["project"]["value"]["fields"]["maintainers"]["value"] =
            node(json!(["/./repo/app"]));
        assert_eq!(
            findings(&repository(declaration, None)),
            [
                (Code::Location, "project.maintainers[0]".to_owned()),
                (Code::Location, "project.summary".to_owned()),
            ]
        );
        assert_eq!(normalized("/repo"), "/repo");
        assert_eq!(normalized("a//b/./c/."), "a/b/c/");
    }

    #[test]
    fn a_manifest_name_that_carries_the_location_or_a_store_path_stays_out_of_the_path() {
        let mut declarations = repository(node(sound()), None);
        *manifest_value(&mut declarations) = serde_json::from_value(node(json!({
            "projects": {"app": "app"},
            "/repo/x": 1,
            "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-x": 2
        })))
        .unwrap();
        let found: Vec<(Code, usize)> = checked(&declarations, ROOT)
            .diagnostics
            .iter()
            .map(|d| (d.code, d.path.len()))
            .collect();
        assert_eq!(found, [(Code::StoreReference, 0), (Code::Location, 0)]);
    }

    #[test]
    fn a_loop_names_every_member_with_its_location() {
        let document = broken();
        let dependencies = document
            .diagnostics
            .iter()
            .find(|d| d.code == Code::Cycle && d.project.as_ref().unwrap().as_str() == "cycle-a")
            .unwrap();
        assert!(
            dependencies
                .message
                .as_str()
                .contains("`cycle-a` depends on `cycle-b`, which depends on `cycle-a`"),
            "{}",
            dependencies.message
        );
        assert_eq!(dependencies.related.len(), 1);
        assert_eq!(
            dependencies.related[0].location.to_string(),
            "projects/cycle-b/project.nix:6:5"
        );
    }

    // a command line never reaches the document, so the checks that keep text out of it don't
    // apply: it may run a program from the store.
    // a command runs what its shell's packages put on the path, and those come from the catalog,
    // so a command line that names a store path runs a package the catalog doesn't hold, whether
    // the path still carries its context or was written as text.
    #[test]
    fn a_command_line_that_names_a_store_path_is_refused_and_one_that_runs_by_name_is_kept_out() {
        let with = |exec: Value| {
            let mut declaration = node(sound());
            declaration["fields"]["commands"] = json!({"at": null, "value": node(json!({
                "run": {"description": "Run it"}
            }))});
            declaration["fields"]["commands"]["value"]["fields"]["run"]["value"]["fields"]["exec"] =
                json!({"at": null, "value": exec});
            checked(&repository(declaration, None), ROOT)
        };
        let store = "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-x/bin/x";
        for context in [true, false] {
            let document = with(json!({"kind": "string", "value": store, "context": context}));
            let codes: Vec<(Code, String)> = document
                .diagnostics
                .iter()
                .map(|d| (d.code, ShowPath(&d.path).to_string()))
                .collect();
            assert_eq!(
                codes,
                [(Code::StoreReference, "commands.run.exec".to_owned())],
                "context {context}"
            );
        }
        let document = with(json!({"kind": "string", "value": "x --all", "context": false}));
        assert_eq!(document.diagnostics, []);
        let app = &document.projects.unwrap()[&Identifier::new("app").unwrap()];
        assert_eq!(app.commands.len(), 1);
    }

    // a package named `java` and a package that runs as `java` both put `java` on the path.
    #[test]
    fn a_package_named_as_another_runs_is_a_conflict() {
        let mut declarations = fixture(include_str!("../fixtures/broken/declarations.json"));
        let catalog = declarations.catalog.as_mut().unwrap();
        let Some(Some(Evaluated::Value(read))) = catalog.systems.get_mut("x86_64-linux") else {
            panic!("the broken fixture's catalog reads on x86_64-linux")
        };
        let jdk25 = read
            .graph
            .iter_mut()
            .find(|(name, _)| name.starts_with("openjdk-25"))
            .map(|(_, package)| package)
            .unwrap();
        jdk25.pname = "java".to_owned();
        jdk25.main_program = None;
        let found = the(&checked(&declarations, ROOT), Code::Conflict, "packages[0]");
        assert!(
            found
                .message
                .as_str()
                .contains("one package named `java` and one that runs as it"),
            "{}",
            found.message
        );
    }

    // a clash between a project and a toolchain it uses lands on the project, which brought it,
    // whatever the two are called: here the importer is renamed to sort before the owner.
    #[test]
    fn a_clash_with_a_toolchain_is_reported_at_the_project_whatever_the_names() {
        let mut declarations = fixture(include_str!("../fixtures/broken/declarations.json"));
        let app = declarations.projects.remove("jvm-app").unwrap();
        declarations.projects.insert("aa-app".to_owned(), app);
        let Node::Attrs { fields } = manifest_value(&mut declarations) else {
            panic!("the broken fixture's manifest is an attribute set")
        };
        let Node::Attrs { fields: listed } = &mut fields.get_mut("projects").unwrap().value else {
            panic!("the broken fixture lists its projects")
        };
        let entry = listed.remove("jvm-app").unwrap();
        listed.insert("aa-app".to_owned(), entry);
        let document = checked(&declarations, ROOT);
        for (code, path) in [
            (Code::Conflict, "packages[0]"),
            (Code::Duplicate, "env.JAVA_HOME"),
        ] {
            let found = the(&document, code, path);
            assert_eq!(
                found.project.as_ref().map(Identifier::as_str),
                Some("aa-app")
            );
            assert_eq!(found.location.file.as_str(), "projects/jvm-app/project.nix");
        }
    }

    #[test]
    fn a_checkout_at_the_filesystem_root_flags_no_string_as_its_location() {
        let declarations = repository(node(sound()), None);
        assert!(checked(&declarations, "/").diagnostics.is_empty());
    }

    #[test]
    fn absent_fields_take_the_published_defaults() {
        let document = checked(&repository(node(sound()), None), ROOT);
        let app = &document.projects.unwrap()[&Identifier::new("app").unwrap()];
        assert!(app.depends_on.is_empty());
        assert!(app.steps.is_empty() && app.commands.is_empty() && app.services.is_empty());
        assert!(app.extensions.is_empty());
        assert_eq!(document.remainder.unwrap().excludes[0].as_str(), "app");
    }

    #[test]
    fn json_data_keeps_numbers_and_nesting() {
        let mut declaration = node(sound());
        declaration["fields"]["extensions"] = json!({"at": null, "value": node(json!({
            "meta": {"count": 3, "ratio": 0.5, "tags": ["a", null, true]}
        }))});
        let mut declarations = repository(declaration, None);
        *manifest_value(&mut declarations) = serde_json::from_value(node(json!({
            "projects": {"app": "app"},
            "extensions": {"meta": {"_tag": "json"}}
        })))
        .unwrap();
        let document = checked(&declarations, ROOT);
        let app = &document.projects.unwrap()[&Identifier::new("app").unwrap()];
        assert_eq!(
            app.extensions[&Identifier::new("meta").unwrap()],
            json!({"count": 3, "ratio": 0.5, "tags": ["a", null, true]})
        );
    }

    #[test]
    fn a_focus_on_an_unlisted_project_is_refused_against_the_manifest() {
        let mut declarations = repository(node(sound()), Some("nowhere"));
        declarations.projects.clear();
        let document = checked(&declarations, ROOT);
        assert_eq!(document.focus, None);
        assert_eq!(document.projects, None);
        let finding = &document.diagnostics[0];
        assert_eq!(finding.code, Code::UnknownReference);
        assert_eq!(finding.location.to_string(), "shikumi.nix");
    }

    #[test]
    fn a_focused_composition_has_its_focus_and_no_remainder() {
        let document = checked(&repository(node(sound()), Some("app")), ROOT);
        assert_eq!(document.focus, Some(Identifier::new("app").unwrap()));
        assert_eq!(document.remainder, None);
        assert!(document.projects.is_some());
    }

    #[test]
    fn a_misspelt_field_suggests_the_one_it_was_likely_meant_to_be() {
        let mut declaration = node(sound());
        declaration["fields"]["project"]["value"]["fields"]["sumary"] =
            json!({"at": null, "value": node(json!("x"))});
        let document = checked(&repository(declaration, None), ROOT);
        let unknown = document
            .diagnostics
            .iter()
            .find(|d| d.code == Code::UnknownOption)
            .unwrap();
        assert_eq!(
            unknown.help.as_ref().map(ToString::to_string).as_deref(),
            Some("did you mean `summary`?")
        );
        assert_eq!(closest("xyz", ["summary"]), None);
    }

    // the checker reads the manifest's fields from their one definition: a misspelling of each is
    // matched against it, and each required one left out is reported, so a field added there
    // reaches the messages with nothing else changed. handling it is a match on the definition's
    // own enum, so the build fails until the checker handles it.
    #[test]
    fn every_manifest_field_reaches_the_checkers_messages() {
        for field in ManifestField::ALL {
            let mut declarations = repository(node(sound()), None);
            let Node::Attrs { fields } = manifest_value(&mut declarations) else {
                panic!("the test's manifest is an attribute set")
            };
            let written = fields.remove("projects").unwrap();
            fields.insert(format!("{}x", field.name()), written);
            let document = checked(&declarations, ROOT);
            let suggested = format!("did you mean `{}`?", field.name());
            assert!(
                document
                    .diagnostics
                    .iter()
                    .any(|d| d.code == Code::UnknownOption
                        && d.help
                            .as_ref()
                            .is_some_and(|help| help.as_str() == suggested)),
                "`{}x` wasn't matched to `{}`: {:#?}",
                field.name(),
                field.name(),
                document.diagnostics
            );
            let missing: Vec<String> = document
                .diagnostics
                .iter()
                .filter(|d| d.code == Code::Missing)
                .map(|d| ShowPath(&d.path).to_string())
                .collect();
            let required: Vec<String> = vocabulary::manifest()
                .fields()
                .iter()
                .filter(|(_, declared)| matches!(declared.presence, Presence::Required))
                .map(|(name, _)| name.clone())
                .collect();
            assert_eq!(missing, required, "{}", field.name());
        }
    }

    // sorted, `a/c` follows `a/b/x` rather than `a`, so a rule that compares neighbours alone
    // misses it.
    #[test]
    fn every_directory_inside_another_is_an_overlap_however_many_share_it() {
        let mut declarations = repository(node(sound()), None);
        *manifest_value(&mut declarations) = serde_json::from_value(node(json!({
            "projects": {"outer": "a", "first": "a/b", "deeper": "a/b/x", "second": "a/c", "apart": "b"}
        })))
        .unwrap();
        declarations.projects.clear();
        let overlaps: Vec<(String, String)> = checked(&declarations, ROOT)
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::Overlap)
            .map(|d| {
                (
                    d.project.as_ref().unwrap().to_string(),
                    d.related[0].message.to_string(),
                )
            })
            .collect();
        assert_eq!(
            overlaps,
            [
                ("first".to_owned(), "`outer` is listed here".to_owned()),
                ("deeper".to_owned(), "`first` is listed here".to_owned()),
                ("second".to_owned(), "`outer` is listed here".to_owned()),
            ]
        );
    }

    #[test]
    fn a_field_without_a_position_is_located_at_its_nearest_enclosing_one() {
        let mut declaration = node(sound());
        declaration["fields"]["project"]["at"] =
            json!({"file": "app/project.nix", "line": 2, "column": 3});
        declaration["fields"]["project"]["value"]["fields"]["maintainers"]["value"] =
            node(json!("someone"));
        let document = checked(&repository(declaration, None), ROOT);
        assert_eq!(
            document.diagnostics[0].location.to_string(),
            "app/project.nix:2:3"
        );
    }
}
