//! the `shikumi` command. it parses arguments and names each subcommand; the work and its errors
//! belong to the crate that owns the subcommand's concern.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "shikumi",
    about = "A build system and project registry that caches in the Nix store"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// every subcommand, one variant each. another component's subcommand is added after the last
/// variant, with its arm in `run`, so no existing entry changes.
#[derive(Debug, Subcommand)]
enum Command {
    #[command(
        about = "Read the registry: the project list, one project, the document, its findings or its schemas"
    )]
    Registry(shikumi_registry::view::RegistryArgs),
}

fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Registry(args) => shikumi_registry::view::run(
            &args,
            &mut std::io::stdout().lock(),
            &mut std::io::stderr().lock(),
        )
        .map_err(|error| error.to_string()),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("shikumi: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_registry_views_parse() {
        for args in [
            &["shikumi", "registry", "list"][..],
            &["shikumi", "registry", "show", "web", "--root", "repo"],
            &["shikumi", "registry", "document", "--focus", "web"],
            &["shikumi", "registry", "schema"],
            &["shikumi", "registry", "schema", "declarations"],
            &["shikumi", "registry", "check", "--format", "json"],
            &["shikumi", "registry", "declarations", "--root", "repo"],
            &["shikumi", "registry", "reference"],
        ] {
            Cli::try_parse_from(args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        }
    }
}
