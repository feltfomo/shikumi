use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufWriter, Write};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use shikumi_bench::baseline::{self, BaselineConfig};
use shikumi_bench::contract::Text;
use shikumi_bench::fixture::overhead::{Shape, UnitGraph};
use shikumi_bench::fixture::rust_workspace::{
    LargeCrateParams, Percent, RustWorkspace, RustWorkspaceParams,
};
use shikumi_bench::measure;
use shikumi_bench::record::Record;

#[derive(Debug, Parser)]
#[command(name = "shikumi-bench", version, about = "Shikumi's benchmark harness")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Generate a fixture", subcommand)]
    Generate(Generate),
    #[command(
        about = "Build generated workspaces with plain Cargo through every scenario and append one record per build"
    )]
    CargoBaseline(BaselineArgs),
    #[command(about = "Check that every line of a records file is a valid record")]
    Validate {
        #[arg(help = "A JSON lines file of records")]
        records: PathBuf,
    },
    #[command(about = "Print the published JSON schemas, or write them into a directory")]
    Schema {
        #[arg(long)]
        write: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum Generate {
    #[command(about = "A seeded Cargo workspace")]
    RustWorkspace {
        #[command(flatten)]
        workspace: WorkspaceArgs,
        #[arg(help = "Directory to create; it must not exist or be empty", long)]
        out: PathBuf,
    },
    #[command(about = "A graph of units whose builders do nothing")]
    Overhead {
        #[arg(long, value_enum)]
        shape: Shape,
        #[arg(long)]
        units: NonZeroU32,
        #[arg(help = "File to write the unit graph to", long)]
        out: PathBuf,
    },
}

#[derive(Debug, Args)]
struct WorkspaceArgs {
    #[arg(long)]
    seed: u64,
    #[arg(long, default_value = "24")]
    crates: NonZeroU32,
    #[arg(long, default_value = "4")]
    depth: NonZeroU32,
    #[arg(long, default_value = "3")]
    dependencies_per_crate: NonZeroU32,
    #[arg(long, default_value = "4")]
    modules_per_crate: NonZeroU32,
    #[arg(long, default_value = "12")]
    functions_per_module: NonZeroU32,
    #[arg(long, default_value_t = 25)]
    generic_percent: u8,
    #[command(flatten)]
    large: Option<LargeArgs>,
}

/// all three or none. each option is optional on its own, so leaving the group out gives `None`;
/// naming any one of them makes the group present, and the group then requires the other two.
#[derive(Debug, Args)]
#[group(multiple = true, requires_all = ["large_modules", "large_functions_per_module", "large_dependencies"])]
struct LargeArgs {
    #[arg(
        help = "Modules in the large crate; leave all three large options out for none",
        long,
        required = false
    )]
    large_modules: NonZeroU32,
    #[arg(long, required = false)]
    large_functions_per_module: NonZeroU32,
    #[arg(long, required = false)]
    large_dependencies: NonZeroU32,
}

#[derive(Debug, Args)]
struct BaselineArgs {
    #[command(flatten)]
    workspace: WorkspaceArgs,
    #[arg(long, default_value_t = 1)]
    iterations: u32,
    #[arg(help = "Scratch directory for the generated workspaces", long)]
    work_dir: PathBuf,
    #[arg(help = "Records file to append to", long)]
    out: PathBuf,
    #[arg(help = "Machine name to record instead of the host name", long)]
    machine: Option<String>,
    #[arg(long, default_value = "cargo")]
    cargo: PathBuf,
}

impl WorkspaceArgs {
    fn generate(&self) -> Result<RustWorkspace, String> {
        let large_crate = self.large.as_ref().map(|large| LargeCrateParams {
            modules: large.large_modules,
            functions_per_module: large.large_functions_per_module,
            dependencies: large.large_dependencies,
        });
        let params = RustWorkspaceParams {
            crates: self.crates,
            depth: self.depth,
            dependencies_per_crate: self.dependencies_per_crate,
            modules_per_crate: self.modules_per_crate,
            functions_per_module: self.functions_per_module,
            generic_percent: Percent::new(self.generic_percent).map_err(|e| e.to_string())?,
            large_crate,
        };
        RustWorkspace::generate(self.seed, params).map_err(|e| e.to_string())
    }
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("shikumi-bench: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Generate(Generate::RustWorkspace { workspace, out }) => {
            let workspace = workspace.generate()?;
            let tree = workspace.render();
            tree.write_new(&out)
                .map_err(|e| format!("writing {}: {e}", out.display()))?;
            println!("{}", tree.digest());
            Ok(())
        }
        Command::Generate(Generate::Overhead { shape, units, out }) => {
            let graph = UnitGraph::generate(shape, units);
            fs::write(&out, graph.to_canonical_json())
                .map_err(|e| format!("writing {}: {e}", out.display()))?;
            println!("{}", graph.digest());
            Ok(())
        }
        Command::CargoBaseline(args) => cargo_baseline(args),
        Command::Validate { records } => validate(&records),
        Command::Schema { write } => schema(write),
    }
}

fn cargo_baseline(args: BaselineArgs) -> Result<(), String> {
    let name = args
        .machine
        .map(Text::new)
        .transpose()
        .map_err(|e| format!("--machine: {e}"))?;
    let config = BaselineConfig {
        workspace: args.workspace.generate()?,
        iterations: args.iterations,
        work_dir: args.work_dir,
        cargo: args.cargo,
        machine: measure::machine(name).map_err(|e| format!("describing this machine: {e}"))?,
    };
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.out)
        .map_err(|e| format!("opening {}: {e}", args.out.display()))?;
    let mut out = BufWriter::new(file);
    baseline::run(&config, &mut |record| {
        out.write_all(record.to_json_line().as_bytes())?;
        // flushed per record so an interrupted run keeps every build it finished.
        out.flush()
    })
    .map_err(|e| e.to_string())
}

fn validate(path: &PathBuf) -> Result<(), String> {
    let file = fs::File::open(path).map_err(|e| format!("opening {}: {e}", path.display()))?;
    let mut count = 0usize;
    let mut failures = 0usize;
    for (index, line) in io::BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| format!("reading {}: {e}", path.display()))?;
        match Record::from_json_line(&line) {
            Ok(_) => count += 1,
            Err(e) => {
                failures += 1;
                eprintln!("{}:{}: {e}", path.display(), index + 1);
            }
        }
    }
    if failures > 0 {
        return Err(format!("{failures} invalid records, {count} valid"));
    }
    if count == 0 {
        return Err(format!("{} holds no records", path.display()));
    }
    println!("{count} valid records");
    Ok(())
}

fn schema(write: Option<PathBuf>) -> Result<(), String> {
    for (name, schema) in shikumi_bench::schemas() {
        let rendered = shikumi_bench::render_schema(&schema);
        match &write {
            Some(dir) => {
                let path = dir.join(name);
                fs::write(&path, rendered)
                    .map_err(|e| format!("writing {}: {e}", path.display()))?;
            }
            None => print!("{rendered}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, Generate};

    fn large_crate(extra: &[&str]) -> Result<bool, clap::Error> {
        let args = ["shikumi-bench", "generate", "rust-workspace", "--seed", "1"];
        let cli = Cli::try_parse_from(args.iter().chain(extra).chain(&["--out", "x"]))?;
        let Command::Generate(Generate::RustWorkspace { workspace, .. }) = cli.command else {
            unreachable!("parsed a different command");
        };
        Ok(workspace.large.is_some())
    }

    #[test]
    fn large_crate_options_come_all_or_none() {
        assert!(!large_crate(&[]).unwrap());
        let all = [
            "--large-modules",
            "2",
            "--large-functions-per-module",
            "3",
            "--large-dependencies",
            "1",
        ];
        assert!(large_crate(&all).unwrap());
        assert!(large_crate(&all[..2]).is_err());
        assert!(large_crate(&all[2..]).is_err());
    }
}
