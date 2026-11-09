//! the repository's own gate checks over its tracked files. the caller supplies the tracked set on
//! stdin, NUL-separated: the flake check lists its source tree, which is exactly what Git tracks,
//! and a developer pipes `git ls-files -z`. both run the same code over the same kind of list.

mod history;
mod links;
mod names;

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read};
use std::process::ExitCode;
use std::thread;

/// how every check reads Markdown, so the links check and the history check see the same document.
const MARKDOWN: pulldown_cmark::Options = pulldown_cmark::Options::ENABLE_TABLES
    .union(pulldown_cmark::Options::ENABLE_FOOTNOTES)
    .union(pulldown_cmark::Options::ENABLE_STRIKETHROUGH)
    .union(pulldown_cmark::Options::ENABLE_TASKLISTS);

const USAGE: &str = "usage: repo-check <links|names|history> < NUL-separated tracked paths\n       \
                     repo-check digest < name";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Links,
    Names,
    /// history markers in the product docs.
    History,
    /// prints the digest line for a name to add to the private-name list.
    Digest,
}

impl Command {
    fn parse(args: &[String]) -> Option<Self> {
        match args {
            [command] => match command.as_str() {
                "links" => Some(Self::Links),
                "names" => Some(Self::Names),
                "history" => Some(Self::History),
                "digest" => Some(Self::Digest),
                _ => None,
            },
            _ => None,
        }
    }
}

/// 1-based line and byte column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Position {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    path: String,
    /// `None` when the finding is about the file as a whole.
    position: Option<Position>,
    message: String,
}

impl Finding {
    pub(crate) fn file(path: &str, message: impl Into<String>) -> Self {
        Self {
            path: path.to_owned(),
            position: None,
            message: message.into(),
        }
    }

    pub(crate) fn at(path: &str, position: Position, message: impl Into<String>) -> Self {
        Self {
            path: path.to_owned(),
            position: Some(position),
            message: message.into(),
        }
    }

    pub(crate) fn position(&self) -> Option<Position> {
        self.position
    }

    #[cfg(test)]
    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            None => write!(f, "{}: {}", self.path, self.message),
            Some(Position { line, column }) => {
                write!(f, "{}:{line}:{column}: {}", self.path, self.message)
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = Command::parse(&args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let findings = match command {
        Command::Digest => return digest(),
        Command::Links => tracked_paths().and_then(|paths| check_links(&paths)),
        Command::Names => tracked_paths().and_then(|paths| check_names(&paths)),
        Command::History => tracked_paths().and_then(|paths| check_history(&paths)),
    };
    match findings {
        Ok(findings) if findings.is_empty() => ExitCode::SUCCESS,
        Ok(findings) => {
            for finding in &findings {
                println!("{finding}");
            }
            eprintln!("repo-check: {} findings", findings.len());
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("repo-check: {error}");
            ExitCode::from(2)
        }
    }
}

fn tracked_paths() -> io::Result<Vec<String>> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    let mut paths = Vec::new();
    for raw in input.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        let path = String::from_utf8(raw.to_vec()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("tracked path {} isn't UTF-8", String::from_utf8_lossy(raw)),
            )
        })?;
        paths.push(path.strip_prefix("./").map(str::to_owned).unwrap_or(path));
    }
    if paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no tracked paths on stdin; a check over nothing would pass vacuously",
        ));
    }
    paths.sort_unstable();
    paths.dedup();
    Ok(paths)
}

fn check_names(paths: &[String]) -> io::Result<Vec<Finding>> {
    let matcher = names::Matcher::protected();
    let per_file = parallel_map(paths, |path| {
        let contents = std::fs::read(path)
            .map_err(|e| io::Error::new(e.kind(), format!("reading {path}: {e}")))?;
        Ok(names::check(&matcher, path, &contents))
    });
    flatten(per_file)
}

fn check_history(paths: &[String]) -> io::Result<Vec<Finding>> {
    let pages: Vec<&String> = paths.iter().filter(|p| history::applies(p)).collect();
    let per_file = parallel_map(&pages, |path| {
        let contents = std::fs::read(path)
            .map_err(|e| io::Error::new(e.kind(), format!("reading {path}: {e}")))?;
        Ok(history::check(path, &contents))
    });
    flatten(per_file)
}

fn check_links(paths: &[String]) -> io::Result<Vec<Finding>> {
    let markdown: Vec<String> = paths
        .iter()
        .filter(|p| links::is_markdown(p))
        .cloned()
        .collect();
    let parsed = parallel_map(&markdown, |path| {
        let contents = std::fs::read(path)
            .map_err(|e| io::Error::new(e.kind(), format!("reading {path}: {e}")))?;
        Ok::<_, io::Error>(links::parse(path, &contents))
    });
    let mut pages = HashMap::with_capacity(markdown.len());
    for (path, page) in markdown.iter().zip(parsed) {
        pages.insert(path.as_str(), page?);
    }
    Ok(links::resolve(paths, &pages))
}

fn digest() -> ExitCode {
    let mut word = String::new();
    if let Err(error) = io::stdin().read_line(&mut word) {
        eprintln!("repo-check: reading the name: {error}");
        return ExitCode::from(2);
    }
    let word = word.trim();
    if word.is_empty() || !word.bytes().all(|b| b.is_ascii_alphabetic()) {
        eprintln!("repo-check: a name is one run of ASCII letters, since that's what a token is");
        return ExitCode::from(2);
    }
    println!("{}", names::digest_line(word));
    ExitCode::SUCCESS
}

fn flatten(per_file: Vec<io::Result<Vec<Finding>>>) -> io::Result<Vec<Finding>> {
    let mut findings = Vec::new();
    for result in per_file {
        findings.extend(result?);
    }
    Ok(findings)
}

/// maps `f` over `items` on every core, keeping the input order so the output is deterministic.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = items.len().div_ceil(workers).max(1);
    thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| scope.spawn(|| part.iter().map(&f).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a check worker panicked"))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_map_keeps_input_order() {
        let items: Vec<u32> = (0..1000).collect();
        assert_eq!(
            parallel_map(&items, |x| x * 2),
            items.iter().map(|x| x * 2).collect::<Vec<_>>()
        );
        assert!(parallel_map(&[] as &[u32], |x| *x).is_empty());
    }

    #[test]
    fn parses_exactly_one_known_command() {
        let parse = |args: &[&str]| {
            Command::parse(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>())
        };
        assert_eq!(parse(&["links"]), Some(Command::Links));
        assert_eq!(parse(&["names"]), Some(Command::Names));
        assert_eq!(parse(&["digest"]), Some(Command::Digest));
        assert_eq!(parse(&["history"]), Some(Command::History));
        assert_eq!(parse(&["link"]), None);
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&["links", "names"]), None);
    }

    #[test]
    fn whole_file_findings_print_without_a_position() {
        let at = Finding::at("a.md", Position { line: 3, column: 5 }, "bad");
        assert_eq!(at.to_string(), "a.md:3:5: bad");
        assert_eq!(Finding::file("a.md", "bad").to_string(), "a.md: bad");
    }
}
