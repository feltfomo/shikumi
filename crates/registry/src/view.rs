//! the read-only registry viewer, as a subcommand a CLI registers: the project list, one project's
//! detail, the whole document, the findings, the serialized declarations, the toolchain context,
//! the published schemas and the declarations reference. it reads only what the producer hands it,
//! so it refuses exactly the declarations every other entry point refuses, and renders their
//! findings against the repository's own files.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::contract::render_json;
use crate::declarations;
use crate::document::{self, Diagnostic, EnvValue, Identifier, Project, Registry, RegistryError};
use crate::evaluation;
use crate::produce::{ProduceError, Producer};
use crate::reference;
use crate::render::{Sources, render};
use crate::toolchain;

#[derive(Debug, Args)]
pub struct RegistryArgs {
    #[command(subcommand)]
    view: View,
}

#[derive(Debug, Subcommand)]
enum View {
    #[command(about = "Print every composed project: its name, directory and summary")]
    List(Composition),
    #[command(about = "Print one project's declarations as the document carries them")]
    Show {
        #[arg(help = "The project to show")]
        project: String,
        #[command(flatten)]
        composition: Composition,
    },
    #[command(about = "Print the registry document")]
    Document(Composition),
    #[command(about = "Check the declarations and print every finding")]
    Check {
        #[arg(
            long,
            value_enum,
            default_value_t = Format::Text,
            help = "Findings as text against the files, or the document that carries them as JSON"
        )]
        format: Format,
        #[command(flatten)]
        composition: Composition,
    },
    #[command(about = "Print the declarations as Shikumi read them, with their source positions")]
    Declarations(Composition),
    #[command(
        about = "Print the toolchain context: each catalog package's store path, for every system, built or not"
    )]
    Context {
        #[arg(
            long,
            default_value = ".",
            help = "The repository root, which holds the manifest"
        )]
        root: PathBuf,
    },
    #[command(about = "Print a published JSON schema")]
    Schema {
        #[arg(
            value_enum,
            default_value_t = Schema::Registry,
            help = "The registry document's schema, the serialized declarations', or an evaluation answer's"
        )]
        of: Schema,
    },
    #[command(about = "Print the declarations reference page")]
    Reference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Schema {
    Registry,
    Declarations,
    Evaluation,
    ToolchainContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Debug, Args)]
struct Composition {
    #[arg(
        long,
        default_value = ".",
        help = "The repository root, which holds the manifest"
    )]
    root: PathBuf,
    #[arg(long, help = "Compose only this project and what it depends on")]
    focus: Option<String>,
}

impl Composition {
    fn focus(&self) -> Result<Option<Identifier>, ViewError> {
        self.focus.as_deref().map(identifier).transpose()
    }

    fn document(&self) -> Result<document::Document, ViewError> {
        Ok(Producer::from_environment()?.document(&self.root, self.focus()?.as_ref())?)
    }

    /// the accepted registry, or every finding rendered to `err` and the refusal.
    fn produce(&self, err: &mut impl Write) -> Result<Registry, ViewError> {
        match Producer::from_environment()?.produce(&self.root, self.focus()?.as_ref()) {
            Err(ProduceError::Document(RegistryError::Refused(findings))) => {
                Err(self.refuse(&findings, err))
            }
            other => Ok(other?),
        }
    }

    fn refuse(&self, findings: &[Diagnostic], to: &mut impl Write) -> ViewError {
        let rendered = render(findings, &mut Sources::new(&self.root));
        match to.write_all(rendered.as_bytes()) {
            Ok(()) => ViewError::Refused(findings.len()),
            Err(error) => ViewError::Write(error),
        }
    }
}

fn identifier(name: &str) -> Result<Identifier, ViewError> {
    Identifier::new(name).map_err(|error| ViewError::Argument(error.to_string()))
}

/// runs one view, printing to `out` and rendering refused findings to `err`. the check view
/// prints its findings to `out`, since they're what it was asked for.
pub fn run(
    args: &RegistryArgs,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<(), ViewError> {
    let text = match &args.view {
        View::List(composition) => list(&composition.produce(err)?),
        View::Show {
            project,
            composition,
        } => {
            let name = identifier(project)?;
            show(&composition.produce(err)?, &name)?
        }
        View::Document(composition) => composition.produce(err)?.to_json(),
        View::Check {
            format,
            composition,
        } => {
            let document = composition.document()?;
            let findings = document.diagnostics();
            match format {
                Format::Json => out
                    .write_all(render_json(&document).as_bytes())
                    .map_err(ViewError::Write)?,
                Format::Text if findings.is_empty() => {}
                Format::Text => return Err(composition.refuse(findings, out)),
            }
            if findings.is_empty() {
                return Ok(());
            }
            return Err(ViewError::Refused(findings.len()));
        }
        View::Declarations(composition) => {
            let producer = Producer::from_environment()?;
            let produced =
                producer.declarations(&composition.root, composition.focus()?.as_ref())?;
            render_json(&produced.declarations)
        }
        View::Context { root } => render_json(&Producer::from_environment()?.context(root)?),
        View::Schema { of } => match of {
            Schema::Registry => render_json(&document::schema()),
            Schema::Declarations => render_json(&declarations::schema()),
            Schema::Evaluation => render_json(&evaluation::schema()),
            Schema::ToolchainContext => render_json(&toolchain::schema()),
        },
        View::Reference => reference::page(),
    };
    out.write_all(text.as_bytes()).map_err(ViewError::Write)
}

/// one line per project, in name order, with the columns aligned.
fn list(registry: &Registry) -> String {
    let projects = registry.projects();
    let name_width = projects.keys().map(|n| n.as_str().len()).max().unwrap_or(0);
    let directory_width = projects
        .values()
        .map(|p| p.directory.as_str().len())
        .max()
        .unwrap_or(0);
    let mut text = String::new();
    for (name, project) in projects {
        writeln!(
            text,
            "{name:name_width$}  {directory:directory_width$}  {summary}",
            directory = project.directory,
            summary = project.summary,
        )
        .unwrap();
    }
    text
}

fn show(registry: &Registry, name: &Identifier) -> Result<String, ViewError> {
    let project = registry
        .projects()
        .get(name)
        .ok_or_else(|| ViewError::NoSuchProject(name.clone()))?;
    Ok(detail(name, project))
}

fn detail(name: &Identifier, project: &Project) -> String {
    let joined = |items: &mut dyn Iterator<Item = &str>| {
        let all: Vec<&str> = items.collect();
        if all.is_empty() {
            "none".to_owned()
        } else {
            all.join(", ")
        }
    };
    let mut text = String::new();
    writeln!(text, "{name}").unwrap();
    writeln!(text, "  directory    {}", project.directory).unwrap();
    writeln!(text, "  summary      {}", project.summary).unwrap();
    let maintainers = joined(&mut project.maintainers.iter().map(|m| m.as_str()));
    writeln!(text, "  maintainers  {maintainers}").unwrap();
    let dependencies = joined(&mut project.depends_on.iter().map(|d| d.as_str()));
    writeln!(text, "  depends on   {dependencies}").unwrap();
    let imports: Vec<String> = project
        .imports
        .iter()
        .map(|(toolchain, owner)| format!("{toolchain} from {owner}"))
        .collect();
    let imports = joined(&mut imports.iter().map(String::as_str));
    writeln!(text, "  imports      {imports}").unwrap();
    let packages = joined(&mut project.packages.iter().map(|p| p.as_str()));
    writeln!(text, "  packages     {packages}").unwrap();

    let section = |text: &mut String, title: &str, rows: Vec<(String, String)>| {
        if rows.is_empty() {
            return;
        }
        writeln!(text, "  {title}").unwrap();
        let width = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        for (row, description) in rows {
            writeln!(text, "    {row:width$}  {description}").unwrap();
        }
    };
    section(
        &mut text,
        "env",
        project
            .env
            .iter()
            .map(|(variable, value)| (variable.to_string(), shown(value)))
            .collect(),
    );
    section(
        &mut text,
        "toolchains",
        project
            .toolchains
            .iter()
            .map(|(name, toolchain)| {
                let mut uses: Vec<String> = Vec::new();
                if !toolchain.packages.is_empty() {
                    let packages: Vec<&str> =
                        toolchain.packages.iter().map(|p| p.as_str()).collect();
                    uses.push(format!("packages {}", packages.join(", ")));
                }
                if !toolchain.env.is_empty() {
                    let variables: Vec<&str> = toolchain.env.keys().map(|v| v.as_str()).collect();
                    uses.push(format!("env {}", variables.join(", ")));
                }
                let uses = if uses.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", uses.join("; "))
                };
                (name.to_string(), format!("{}{uses}", toolchain.description))
            })
            .collect(),
    );
    section(
        &mut text,
        "steps",
        project
            .steps
            .iter()
            .map(|(step, s)| {
                let after = if s.after.is_empty() {
                    String::new()
                } else {
                    let after: Vec<&str> = s.after.iter().map(|a| a.as_str()).collect();
                    format!(" (after {})", after.join(", "))
                };
                (step.to_string(), format!("{}{after}", s.description))
            })
            .collect(),
    );
    section(
        &mut text,
        "commands",
        project
            .commands
            .iter()
            .map(|(command, c)| (command.to_string(), c.description.to_string()))
            .collect(),
    );
    section(
        &mut text,
        "services",
        project
            .services
            .iter()
            .map(|(service, s)| {
                (
                    service.to_string(),
                    format!("{} (restart {})", s.description, s.restart),
                )
            })
            .collect(),
    );
    section(
        &mut text,
        "extensions",
        project
            .extensions
            .iter()
            .map(|(namespace, value)| (namespace.to_string(), value.to_string()))
            .collect(),
    );
    text
}

// a variable's value as a person reads it: text quoted, so an empty one shows, or the catalog
// package it points into.
fn shown(value: &EnvValue) -> String {
    match value {
        EnvValue::Text(text) => format!("{text:?}"),
        EnvValue::Package(location) => match &location.path {
            None => format!("package {}", location.package),
            Some(path) => format!("package {}, at {path}", location.package),
        },
    }
}

#[derive(Debug)]
pub enum ViewError {
    Argument(String),
    NoSuchProject(Identifier),
    Produce(ProduceError),
    /// the declarations are refused; their findings are already rendered.
    Refused(usize),
    Write(io::Error),
}

impl From<ProduceError> for ViewError {
    fn from(error: ProduceError) -> Self {
        Self::Produce(error)
    }
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Argument(message) => f.write_str(message),
            Self::NoSuchProject(name) => write!(f, "no project `{name}` in the composition"),
            Self::Produce(error) => error.fmt(f),
            Self::Refused(count) => f.write_str(&document::refusal_summary(*count)),
            Self::Write(error) => write!(f, "writing the output: {error}"),
        }
    }
}

impl std::error::Error for ViewError {}

#[cfg(test)]
mod tests {
    use super::*;

    // the document is what the packaged `shikumi` produces from the declaration layer's basic
    // fixture repository on every Nix implementation, which the `registry-no-builds-*` checks hold.
    fn basic() -> Registry {
        Registry::from_json(include_bytes!("../fixtures/basic/document.json")).unwrap()
    }

    #[test]
    fn the_list_prints_every_project_with_aligned_columns() {
        assert_eq!(list(&basic()), include_str!("../fixtures/basic/list.txt"));
    }

    #[test]
    fn one_project_prints_every_record_it_declares() {
        let web = Identifier::new("web").unwrap();
        assert_eq!(
            show(&basic(), &web).unwrap(),
            include_str!("../fixtures/basic/show-web.txt")
        );
    }

    #[test]
    fn showing_a_project_outside_the_composition_fails() {
        let missing = Identifier::new("nowhere").unwrap();
        assert!(matches!(
            show(&basic(), &missing),
            Err(ViewError::NoSuchProject(_))
        ));
    }

    #[test]
    fn a_name_that_is_not_an_identifier_is_refused_before_composing() {
        let args = RegistryArgs {
            view: View::Show {
                project: "Web".to_owned(),
                composition: Composition {
                    root: PathBuf::from("."),
                    focus: None,
                },
            },
        };
        assert!(matches!(
            run(&args, &mut Vec::new(), &mut Vec::new()),
            Err(ViewError::Argument(_))
        ));
    }
}
