//! the docs history check: product docs describe the system as it is, so a page under `docs/`
//! fails when its prose carries a marker of history. `docs/records/`, which keeps evidence of what
//! happened, is exempt. the markers are the ones a page picks up when a change is appended to it
//! rather than rewritten into it:
//!
//! - a plan's phase or band number, such as "phase 2.1", "Band 1a" or "phase-0.3";
//! - a date, written as `2026-11-09` or as a month and a year;
//! - an effort, the planning unit a page has no reason to name;
//! - changelog phrasing, such as "as of", "previously", "no longer" or "added in", which reads as a
//!   delta against an earlier version the reader never saw.
//!
//! prose is read through the Markdown parser, so a phrase broken across lines is still one phrase,
//! and fenced code, link destinations and HTML aren't prose. inline code is, since a date or a
//! phase label in backticks is as much history as one outside them.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use crate::{Finding, Position};

/// changelog phrasing. each says what changed rather than what is: "no longer" and "now also"
/// against a former behavior, "as of", "added in", "introduced in", "since version" and
/// "deprecated in" against a version or date, "previously", "formerly", "originally" and "until
/// recently" against an earlier state, "used to be", "was renamed" and the "has been" and "was"
/// forms of replaced and removed against a change itself. "removed in", "changed in" and "new in"
/// stay out: "removed in the same change" states a rule in the present.
const PHRASES: &[&str] = &[
    "as of",
    "previously",
    "no longer",
    "now also",
    "added in",
    "introduced in",
    "since version",
    "deprecated in",
    "formerly",
    "originally",
    "until recently",
    "used to be",
    "was renamed",
    "has been replaced",
    "was replaced",
    "has been removed",
    "was removed",
];

/// words a plan's stage numbers follow.
const STAGES: &[&str] = &["phase", "phases", "band", "bands"];

const EFFORTS: &[&str] = &["effort", "efforts"];

const MONTHS: &[&str] = &[
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

/// whether `path` is a page this check reads.
pub(crate) fn applies(path: &str) -> bool {
    path.starts_with("docs/")
        && !path.starts_with("docs/records/")
        && crate::links::is_markdown(path)
}

/// every history marker in one page's prose.
pub(crate) fn check(path: &str, contents: &[u8]) -> Vec<Finding> {
    let Ok(text) = std::str::from_utf8(contents) else {
        return vec![Finding::file(path, "isn't UTF-8")];
    };
    let mut findings = Vec::new();
    let mut prose = Prose::default();
    let mut code_block = false;
    for (event, range) in Parser::new_ext(text, crate::MARKDOWN).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code_block = true,
            Event::End(TagEnd::CodeBlock) => code_block = false,
            Event::Text(fragment) | Event::Code(fragment) if !code_block => {
                prose.push(&fragment, range.start);
            }
            Event::SoftBreak | Event::HardBreak => prose.push(" ", range.start),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::TableCell
                | TagEnd::Item
                | TagEnd::FootnoteDefinition,
            ) => prose.flush(path, text, &mut findings),
            _ => {}
        }
    }
    prose.flush(path, text, &mut findings);
    findings.sort_by_key(Finding::position);
    findings
}

/// one block's prose, joined, with where each piece came from in the file.
#[derive(Debug, Default)]
struct Prose {
    text: String,
    /// (offset in `text`, offset in the file) for the start of each piece.
    pieces: Vec<(usize, usize)>,
}

impl Prose {
    fn push(&mut self, fragment: &str, source: usize) {
        self.pieces.push((self.text.len(), source));
        self.text.push_str(fragment);
    }

    fn flush(&mut self, path: &str, file: &str, findings: &mut Vec<Finding>) {
        for (offset, marker) in markers(&self.text) {
            let piece = self.pieces.partition_point(|&(start, _)| start <= offset) - 1;
            let (start, source) = self.pieces[piece];
            // a piece's text is its source unless the parser unescaped it, so the offset into it
            // is exact for plain prose and the piece's start for the rest.
            let at = if file
                .get(source..)
                .is_some_and(|rest| rest.starts_with(&self.text[start..offset.max(start)]))
            {
                source + offset - start
            } else {
                source
            };
            findings.push(Finding::at(
                path,
                locate(file, at),
                format!(
                    "\"{marker}\" is a history marker; a page describes the present, and history belongs in docs/records/ or Git"
                ),
            ));
        }
        self.text.clear();
        self.pieces.clear();
    }
}

fn locate(text: &str, offset: usize) -> Position {
    let before = &text[..offset];
    Position {
        line: before.matches('\n').count() + 1,
        column: offset - before.rfind('\n').map_or(0, |i| i + 1) + 1,
    }
}

/// the words of `text`, each with its byte offset; a word is a run of ASCII letters and digits,
/// with `-`, `.` and `/` kept inside it so dates, versions and branch names stay whole. they're
/// compared ignoring ASCII case, so none is copied to lowercase it.
fn words(text: &str) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let mut start = None;
    let bytes = text.as_bytes();
    let part = |b: u8| b.is_ascii_alphanumeric();
    for (index, &byte) in bytes.iter().enumerate() {
        let inner = matches!(byte, b'-' | b'.' | b'/')
            && start.is_some()
            && bytes.get(index + 1).is_some_and(|&next| part(next));
        match (start, part(byte) || inner) {
            (None, true) => start = Some(index),
            (Some(from), false) => {
                words.push((from, &text[from..index]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        words.push((from, &text[from..]));
    }
    words
}

fn one_of(word: &str, list: &[&str]) -> bool {
    list.iter().any(|item| item.eq_ignore_ascii_case(word))
}

fn strip_prefix_ignoring_case<'w>(word: &'w str, prefix: &str) -> Option<&'w str> {
    let head = word.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &word[prefix.len()..])
}

fn is_year(word: &str) -> bool {
    word.len() == 4 && word.bytes().all(|b| b.is_ascii_digit()) && word.starts_with(['1', '2'])
}

// `2026-11-09` or `2026-11`.
fn is_iso_date(word: &str) -> bool {
    let mut parts = word.split('-');
    let year = parts.next().is_some_and(is_year);
    let rest: Vec<&str> = parts.collect();
    year && (1..=2).contains(&rest.len())
        && rest
            .iter()
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// each marker in `text` with its byte offset.
fn markers(text: &str) -> Vec<(usize, String)> {
    let words = words(text);
    let mut found = Vec::new();
    for (index, (offset, word)) in words.iter().enumerate() {
        let next = words.get(index + 1).map(|(_, w)| *w);
        let starts_with_digit =
            |w: Option<&str>| w.is_some_and(|w| w.starts_with(|c: char| c.is_ascii_digit()));
        // a marker that is one word on its own: a date, a label such as `phase-0.3`, or an effort.
        let labelled = STAGES.iter().any(|stage| {
            strip_prefix_ignoring_case(word, stage)
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(|rest| starts_with_digit(Some(rest)))
        });
        let effort = one_of(word, EFFORTS) || strip_prefix_ignoring_case(word, "effort/").is_some();
        if is_iso_date(word) || labelled || effort {
            found.push((*offset, text[*offset..*offset + word.len()].to_owned()));
        } else if one_of(word, MONTHS) && next.is_some_and(is_year) {
            let end = words[index + 1].0 + 4;
            found.push((*offset, text[*offset..end].to_owned()));
        } else if one_of(word, STAGES) && starts_with_digit(next) {
            let (next_offset, next_word) = &words[index + 1];
            found.push((
                *offset,
                text[*offset..next_offset + next_word.len()].to_owned(),
            ));
        }
    }
    let lower = text.to_ascii_lowercase();
    let boundary = |b: Option<&u8>| b.is_none_or(|b| !b.is_ascii_alphanumeric());
    for phrase in PHRASES {
        for (offset, _) in lower.match_indices(phrase) {
            let end = offset + phrase.len();
            if boundary(offset.checked_sub(1).and_then(|i| lower.as_bytes().get(i)))
                && boundary(lower.as_bytes().get(end))
            {
                found.push((offset, text[offset..end].to_owned()));
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(page: &str) -> Vec<String> {
        check("docs/page.md", page.as_bytes())
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn markers_in(page: &str) -> Vec<String> {
        check("docs/page.md", page.as_bytes())
            .iter()
            .map(|f| {
                f.message()
                    .split('"')
                    .nth(1)
                    .expect("a finding quotes its marker")
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn each_kind_of_marker_is_caught() {
        for (page, marker) in [
            ("Built in phase 2.1.\n", "phase 2.1"),
            ("Band 1a owns it.\n", "Band 1a"),
            ("Ruled on 2026-11-09.\n", "2026-11-09"),
            ("Measured in November 2026.\n", "November 2026"),
            ("The declarations effort adds it.\n", "effort"),
            ("Merged from effort/declarations.\n", "effort/declarations"),
            ("As of this release it works.\n", "As of"),
            ("It previously failed.\n", "previously"),
            ("It no longer builds.\n", "no longer"),
            ("It now also checks paths.\n", "now also"),
            ("Added in the last release.\n", "Added in"),
            ("It was renamed.\n", "was renamed"),
        ] {
            assert_eq!(markers_in(page), [marker], "{page:?}");
        }
    }

    #[test]
    fn a_phrase_broken_across_lines_is_still_caught_where_it_starts() {
        let found = found("The pinned nixpkgs no\nlonger builds for it.\n");
        assert_eq!(found.len(), 1);
        assert!(
            found[0].starts_with("docs/page.md:1:20: \"no longer\" is a history marker"),
            "{found:?}"
        );
    }

    #[test]
    fn markers_in_inline_code_and_table_cells_are_caught() {
        assert_eq!(markers_in("Run `phase-0.3` checks.\n"), ["phase-0.3"]);
        assert_eq!(markers_in("See `2026-11-09`.\n"), ["2026-11-09"]);
        assert_eq!(
            markers_in("| a | b |\n| - | - |\n| x | as of now |\n"),
            ["as of"]
        );
    }

    #[test]
    fn code_blocks_and_link_destinations_are_not_prose() {
        let page = "```text\nphase 2.1 on 2026-11-09\n```\n\n[the plan](plans/2026-11-09.md)\n";
        assert!(found(page).is_empty(), "{:?}", found(page));
    }

    #[test]
    fn present_tense_prose_passes() {
        let page = "The phase of the moon, a band of 3 colours read as phases. \
                    Lix 2.95 and version 1.0 work, and the store holds 2026 paths. \
                    A root longer than a path is a typo, as often happens. \
                    The old path is removed in the same change.\n";
        assert!(found(page).is_empty(), "{:?}", found(page));
    }

    #[test]
    fn only_pages_under_docs_outside_the_records_are_read() {
        assert!(applies("docs/reference/capabilities.md"));
        assert!(!applies("docs/records/2026-gate.md"));
        assert!(!applies("README.md"));
        assert!(!applies("docs/diagram.svg"));
    }
}
