//! findings rendered for people, the way rustc renders its errors: the code and the sentence, the
//! file, line and column, the line of the user's file with the field underlined, and a fix where
//! there is one. the same findings serialize as JSON inside the registry document; this is only
//! their text form.
//!
//! written here rather than taken from a snippet crate because every span is one attribute name on
//! one line, which the evaluator places by its start alone. underlining that name, given the
//! field's path, is most of the work, and no crate knows Nix's attribute paths. the rendered bytes
//! are a tracked golden held on every Nix implementation, so they change only when this file does,
//! not when a dependency's layout does. display widths come from `unicode-width`, the table the
//! snippet crates use themselves.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::document::{Diagnostic, Location, Position, Segment};

/// the lines of the repository's files, read once each and only when a finding points into them.
#[derive(Debug)]
pub struct Sources {
    root: PathBuf,
    files: HashMap<String, Option<Vec<String>>>,
}

impl Sources {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            files: HashMap::new(),
        }
    }

    // a file that can't be read renders without its snippet rather than failing the report.
    fn line(&mut self, file: &str, line: usize) -> Option<&str> {
        let root = &self.root;
        let lines = self.files.entry(file.to_owned()).or_insert_with(|| {
            std::fs::read_to_string(root.join(file)).ok().map(|text| {
                // the evaluator ends a line at `\n`, `\r\n` or a lone `\r`.
                text.replace("\r\n", "\n")
                    .split(['\n', '\r'])
                    .map(str::to_owned)
                    .collect()
            })
        });
        lines
            .as_ref()?
            .get(line.checked_sub(1)?)
            .map(String::as_str)
    }
}

pub fn render(diagnostics: &[Diagnostic], sources: &mut Sources) -> String {
    let mut text = String::new();
    for diagnostic in diagnostics {
        render_one(&mut text, diagnostic, sources);
        text.push('\n');
    }
    text
}

fn render_one(text: &mut String, diagnostic: &Diagnostic, sources: &mut Sources) {
    let lines = std::iter::once(&diagnostic.location)
        .chain(diagnostic.related.iter().map(|related| &related.location))
        .filter_map(|location| location.position.map(|p| p.line.get()))
        .max()
        .unwrap_or(0);
    let gutter = " ".repeat(lines.max(1).to_string().len());
    writeln!(
        text,
        "error[{}]: {}",
        diagnostic.code,
        diagnostic.headline()
    )
    .unwrap();
    let names = names(&diagnostic.path);
    snippet(text, &gutter, &diagnostic.location, &names, sources);
    if let Some(help) = &diagnostic.help {
        writeln!(text, "{gutter} = help: {help}").unwrap();
    }
    for related in &diagnostic.related {
        writeln!(text, "note: {}", related.message).unwrap();
        snippet(text, &gutter, &related.location, &names, sources);
    }
}

// the attribute names on a finding's path, innermost first, which the underline looks for.
fn names(path: &[Segment]) -> Vec<&str> {
    path.iter()
        .rev()
        .filter_map(|segment| match segment {
            Segment::Field(name) => Some(name.as_str()),
            Segment::Index(_) => None,
        })
        .collect()
}

fn snippet(
    text: &mut String,
    gutter: &str,
    location: &Location,
    names: &[&str],
    sources: &mut Sources,
) {
    writeln!(text, "{gutter}--> {location}").unwrap();
    let Some(Position { line, column }) = location.position else {
        return;
    };
    let Some(source) = sources.line(location.file.as_str(), line.get() as usize) else {
        return;
    };
    let width = gutter.len();
    writeln!(text, "{gutter} |").unwrap();
    writeln!(text, "{:>width$} | {source}", line.get()).unwrap();
    if let Some((start, name)) = underline(source, column.get() as usize, names) {
        // the gap keeps the line's tabs, so the carets sit under the name in any tab width, and
        // gives every other character the columns a terminal draws it in: two for a wide one,
        // such as a CJK character or most emoji, and none for a combining mark.
        let mut gap = String::with_capacity(start);
        for c in source[..start].chars() {
            if c == '\t' {
                gap.push('\t');
            } else {
                gap.extend(std::iter::repeat_n(' ', c.width().unwrap_or(0)));
            }
        }
        writeln!(text, "{gutter} | {gap}{}", "^".repeat(name.width())).unwrap();
    }
}

/// the byte offset of the name to underline, and the name. the evaluator gives the column,
/// counted in bytes from 1, where the attribute path that defined the field starts, so a field set
/// as `steps.build.exec` has the position of `steps`. the innermost name of the finding's path that
/// appears in that attribute path is underlined; failing that, the attribute path's first name.
/// where no name starts at the column, as at the token a syntax error stopped on, the one
/// character there is.
fn underline<'s>(source: &'s str, column: usize, names: &[&str]) -> Option<(usize, &'s str)> {
    let start = column.checked_sub(1)?;
    let rest = source.get(start..)?;
    let length = rest
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '\'' | '-' | '.')))
        .unwrap_or(rest.len());
    let attribute_path = &rest[..length];
    let mut offset = 0;
    let mut segments = Vec::new();
    for segment in attribute_path.split('.') {
        segments.push((start + offset, segment));
        offset += segment.len() + 1;
    }
    let (at, segment) = names
        .iter()
        .find_map(|name| segments.iter().find(|(_, segment)| segment == name))
        .or_else(|| segments.first())
        .copied()?;
    if segment.is_empty() {
        let character = rest.chars().next()?;
        return Some((start, &rest[..character.len_utf8()]));
    }
    Some((at, segment))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::document::{Code, Related, RelativePath, Severity, Text};

    fn at(file: &str, line: u32, column: u32) -> Location {
        Location {
            file: RelativePath::new(file).unwrap(),
            position: Some(Position {
                line: NonZeroU32::new(line).unwrap(),
                column: NonZeroU32::new(column).unwrap(),
            }),
        }
    }

    fn diagnostic(path: &[&str], location: Location) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            code: Code::Missing,
            project: None,
            path: path
                .iter()
                .map(|s| Segment::Field((*s).to_owned()))
                .collect(),
            location,
            message: Text::new("is required but not defined").unwrap(),
            help: Some(Text::new("add `exec = ...;`").unwrap()),
            related: Vec::new(),
        }
    }

    fn sources(files: &[(&str, &str)]) -> (tempfile::TempDir, Sources) {
        let root = tempfile::tempdir().unwrap();
        for (file, text) in files {
            let path = root.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let sources = Sources::new(root.path());
        (root, sources)
    }

    #[test]
    fn a_finding_underlines_its_field_in_the_users_file() {
        let (_root, mut sources) = sources(&[(
            "app/project.nix",
            "{\n  steps.build.description = \"Has no command\";\n}\n",
        )]);
        let found = diagnostic(&["steps", "build", "exec"], at("app/project.nix", 2, 3));
        assert_eq!(
            render(&[found], &mut sources),
            "error[missing]: `steps.build.exec` is required but not defined\n \
             --> app/project.nix:2:3\n  |\n2 |   steps.build.description = \"Has no command\";\n  |         ^^^^^\n  = help: add `exec = ...;`\n\n"
        );
    }

    #[test]
    fn tabs_before_the_field_stay_tabs_under_it() {
        let (_root, mut sources) = sources(&[("a.nix", "{\n\tsummary = 1;\n}\n")]);
        let rendered = render(&[diagnostic(&["summary"], at("a.nix", 2, 2))], &mut sources);
        assert!(rendered.contains("  | \t^^^^^^^\n"), "{rendered}");
    }

    #[test]
    fn the_carets_sit_under_the_name_after_wide_and_combining_characters() {
        // `日本語` and the crab take two columns each, and the accent on `é` none.
        let line = "{ a = \"日本語🦀e\u{301}\"; summary = 1; }";
        let column = u32::try_from(line.find("summary").unwrap() + 1).unwrap();
        let (_root, mut sources) = sources(&[("a.nix", &format!("{line}\n"))]);
        let rendered = render(
            &[diagnostic(&["summary"], at("a.nix", 1, column))],
            &mut sources,
        );
        let gap = " ".repeat("{ a = \"".len() + 9 + "\"; ".len());
        assert!(
            rendered.contains(&format!("\n  | {gap}^^^^^^^\n")),
            "{rendered}"
        );
    }

    #[test]
    fn a_line_ends_at_a_lone_carriage_return_as_the_evaluator_counts_it() {
        let (_root, mut sources) = sources(&[("a.nix", "{\r  summary = 1;\r}\r")]);
        let rendered = render(&[diagnostic(&["summary"], at("a.nix", 2, 3))], &mut sources);
        assert!(
            rendered.contains("2 |   summary = 1;\n  |   ^^^^^^^\n"),
            "{rendered}"
        );
    }

    #[test]
    fn a_file_level_or_unreadable_location_renders_without_a_snippet() {
        let (_root, mut sources) = sources(&[]);
        let whole = Location {
            file: RelativePath::new("gone/project.nix").unwrap(),
            position: None,
        };
        let rendered = render(
            &[
                diagnostic(&[], whole),
                diagnostic(&["a"], at("gone/project.nix", 9, 1)),
            ],
            &mut sources,
        );
        assert!(!rendered.contains(" | "), "{rendered}");
        assert!(rendered.contains("--> gone/project.nix\n"), "{rendered}");
        assert!(
            rendered.contains("--> gone/project.nix:9:1\n"),
            "{rendered}"
        );
    }

    #[test]
    fn a_related_location_is_a_note_with_its_own_snippet() {
        let (_root, mut sources) = sources(&[
            ("a/project.nix", "{\n  commands.fmt = { };\n}\n"),
            (
                "b/project.nix",
                "{\n\n\n\n\n\n\n\n\n  commands.fmt = { };\n}\n",
            ),
        ]);
        let mut found = diagnostic(&["commands", "fmt"], at("b/project.nix", 10, 3));
        found.related.push(Related {
            location: at("a/project.nix", 2, 3),
            message: Text::new("`a` declares it here").unwrap(),
        });
        let rendered = render(&[found], &mut sources);
        assert!(
            rendered.contains("note: `a` declares it here\n  --> a/project.nix:2:3\n   |\n 2 |   commands.fmt = { };\n   |            ^^^\n"),
            "{rendered}"
        );
    }
}
