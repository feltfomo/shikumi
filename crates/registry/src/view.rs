//! the read-only registry viewer, as a subcommand a CLI registers: the project list, one project's
//! detail, the whole document, or the published schema. it reads only what the producer hands it,
//! so it refuses exactly the declarations every other entry point refuses.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::document::{self, Identifier, Project, Registry};
use crate::produce::{ProduceError, Producer};

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
    #[command(about = "Print the registry document's JSON schema")]
    Schema,
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
    fn produce(&self) -> Result<Registry, ViewError> {
        let focus = self.focus.as_deref().map(identifier).transpose()?;
        Ok(Producer::from_environment()?.produce(&self.root, focus.as_ref())?)
    }
}

fn identifier(name: &str) -> Result<Identifier, ViewError> {
    Identifier::new(name).map_err(|error| ViewError::Argument(error.to_string()))
}

pub fn run(args: &RegistryArgs, out: &mut impl Write) -> Result<(), ViewError> {
    let text = match &args.view {
        View::List(composition) => list(&composition.produce()?),
        View::Show {
            project,
            composition,
        } => {
            let name = identifier(project)?;
            show(&composition.produce()?, &name)?
        }
        View::Document(composition) => composition.produce()?.to_json(),
        View::Schema => document::render_schema(&document::schema()),
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

#[derive(Debug)]
pub enum ViewError {
    Argument(String),
    NoSuchProject(Identifier),
    Produce(ProduceError),
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
            Self::Write(error) => write!(f, "writing the output: {error}"),
        }
    }
}

impl std::error::Error for ViewError {}

#[cfg(test)]
mod tests {
    use super::*;

    // the fixtures are the declaration layer's own output for its basic fixture repository; the
    // gate's `registry-viewer` check produces them again and fails if they differ.
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
            run(&args, &mut Vec::new()),
            Err(ViewError::Argument(_))
        ));
    }
}
