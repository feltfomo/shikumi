//! the link check: every relative link in a tracked Markdown file must reach a tracked file or a
//! directory holding one, and a fragment pointing into a Markdown file must name one of its
//! headings or HTML anchors. links with a scheme are someone else's to keep alive and are skipped.

use std::collections::{HashMap, HashSet};

use pulldown_cmark::{BrokenLink, Event, LinkType, Parser, Tag, TagEnd};

use crate::{Finding, Position};

/// what one Markdown file offers to the others and asks of them.
#[derive(Debug, Default)]
pub(crate) struct Page {
    anchors: HashSet<String>,
    links: Vec<Link>,
    findings: Vec<Finding>,
}

#[derive(Debug)]
struct Link {
    destination: String,
    position: Position,
}

pub(crate) fn is_markdown(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("md"))
}

/// parses one file. kept separate from resolution so every page can be parsed in parallel before
/// any link needs another page's anchors.
pub(crate) fn parse(path: &str, contents: &[u8]) -> Page {
    let mut page = Page::default();
    let text = match std::str::from_utf8(contents) {
        Ok(text) => text,
        Err(error) => {
            page.findings
                .push(Finding::file(path, format!("isn't UTF-8 ({error})")));
            return page;
        }
    };
    let lines = LineIndex::new(text);
    let mut broken = Vec::new();
    let mut on_broken = |link: BrokenLink<'_>| {
        // a full or collapsed reference with no definition is a link the author meant and lost; a
        // shortcut `[like this]` is as often plain bracketed prose, so it isn't reported.
        if matches!(link.link_type, LinkType::Reference | LinkType::Collapsed) {
            broken.push((link.span.start, link.reference.to_string()));
        }
        None
    };
    let parser = Parser::new_with_broken_link_callback(text, crate::MARKDOWN, Some(&mut on_broken));

    let mut slugs = Slugger::default();
    let mut heading: Option<String> = None;
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { .. }) => heading = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => {
                if let Some(text) = heading.take() {
                    page.anchors.insert(slugs.slug(&text));
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some(heading) = heading.as_mut() {
                    heading.push_str(&t);
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                page.anchors.extend(html_anchors(&html));
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            })
            | Event::Start(Tag::Image {
                link_type,
                dest_url,
                ..
            }) if !matches!(link_type, LinkType::Email) => {
                page.links.push(Link {
                    destination: dest_url.into_string(),
                    position: lines.locate(range.start),
                });
            }
            _ => {}
        }
    }
    for (offset, reference) in broken {
        page.findings.push(Finding::at(
            path,
            lines.locate(offset),
            format!("reference [{reference}] has no definition"),
        ));
    }
    page
}

/// resolves every page's links against the tracked set and the parsed anchors.
pub(crate) fn resolve(tracked: &[String], pages: &HashMap<&str, Page>) -> Vec<Finding> {
    let files: HashSet<&str> = tracked.iter().map(String::as_str).collect();
    let mut directories: HashSet<&str> = HashSet::new();
    for path in tracked {
        let mut rest = path.as_str();
        while let Some((parent, _)) = rest.rsplit_once('/') {
            if !directories.insert(parent) {
                break;
            }
            rest = parent;
        }
    }

    let mut findings = Vec::new();
    for path in tracked {
        let Some(page) = pages.get(path.as_str()) else {
            continue;
        };
        let first = findings.len();
        findings.extend(page.findings.iter().cloned());
        for link in &page.links {
            if let Err(message) = check_link(path, &link.destination, &files, &directories, pages) {
                findings.push(Finding::at(path, link.position, message));
            }
        }
        findings[first..].sort_by_key(Finding::position);
    }
    findings
}

fn check_link(
    from: &str,
    destination: &str,
    files: &HashSet<&str>,
    directories: &HashSet<&str>,
    pages: &HashMap<&str, Page>,
) -> Result<(), String> {
    if has_scheme(destination) || destination.starts_with("//") {
        return Ok(());
    }
    let (target, fragment) = match destination.split_once('#') {
        Some((target, fragment)) => (target, Some(fragment)),
        None => (destination, None),
    };
    let target = target.split_once('?').map_or(target, |(path, _)| path);
    let target = percent_decode(target).ok_or_else(|| format!("{destination}: bad escape"))?;

    let resolved = if target.is_empty() {
        from.to_owned()
    } else {
        let base = if target.starts_with('/') {
            ""
        } else {
            from.rsplit_once('/').map_or("", |(dir, _)| dir)
        };
        normalize(base, &target).ok_or_else(|| format!("{destination} leaves the repository"))?
    };
    let wants_directory = target.ends_with('/');
    let is_file = files.contains(resolved.as_str());
    let is_directory = resolved.is_empty() || directories.contains(resolved.as_str());
    if !(is_directory || is_file && !wants_directory) {
        return Err(format!("{destination} doesn't reach a tracked path"));
    }

    if let Some(fragment) = fragment.filter(|f| !f.is_empty())
        && is_file
        && let Some(page) = pages.get(resolved.as_str())
    {
        let anchor =
            percent_decode(fragment).ok_or_else(|| format!("{destination}: bad escape"))?;
        if !page.anchors.contains(&anchor) {
            return Err(format!("{destination}: {resolved} has no anchor #{anchor}"));
        }
    }
    Ok(())
}

fn has_scheme(destination: &str) -> bool {
    let Some((scheme, _)) = destination.split_once(':') else {
        return false;
    };
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

/// joins `target` onto `base` and resolves `.` and `..`; `None` when `..` climbs above the root.
fn normalize(base: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `id` and `name` attribute values in a fragment of HTML, which GitHub also accepts as anchors.
fn html_anchors(html: &str) -> Vec<String> {
    let mut anchors = Vec::new();
    for attribute in ["id=\"", "name=\""] {
        let mut rest = html;
        while let Some(at) = rest.find(attribute) {
            let preceded_by_space = at == 0 || rest.as_bytes()[at - 1].is_ascii_whitespace();
            rest = &rest[at + attribute.len()..];
            if let Some(end) = rest.find('"')
                && preceded_by_space
            {
                anchors.push(rest[..end].to_owned());
            }
        }
    }
    anchors
}

/// GitHub's heading anchors: lowercase, punctuation dropped except `-` and `_`, spaces turned into
/// hyphens, and a repeated heading numbered from `-1`.
#[derive(Debug, Default)]
struct Slugger {
    seen: HashMap<String, usize>,
}

impl Slugger {
    fn slug(&mut self, heading: &str) -> String {
        let base: String = heading
            .trim()
            .chars()
            .flat_map(char::to_lowercase)
            .filter_map(|c| match c {
                ' ' => Some('-'),
                '-' | '_' => Some(c),
                c if c.is_alphanumeric() => Some(c),
                _ => None,
            })
            .collect();
        let mut slug = base.clone();
        let count = self.seen.entry(base.clone()).or_insert(0);
        if *count > 0 {
            slug = format!("{base}-{count}");
        }
        *count += 1;
        // a numbered slug can collide with a later literal heading of the same text.
        self.seen.entry(slug.clone()).or_insert(1);
        slug
    }
}

struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { starts }
    }

    /// 1-based line and byte column of a byte offset.
    fn locate(&self, offset: usize) -> Position {
        let line = self.starts.partition_point(|&start| start <= offset);
        Position {
            line,
            column: offset - self.starts[line - 1] + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(files: &[(&str, &str)]) -> Vec<String> {
        let tracked: Vec<String> = files.iter().map(|(p, _)| (*p).to_owned()).collect();
        let pages: HashMap<&str, Page> = files
            .iter()
            .filter(|(p, _)| is_markdown(p))
            .map(|(p, c)| (*p, parse(p, c.as_bytes())))
            .collect();
        resolve(&tracked, &pages)
            .into_iter()
            .map(|f| f.to_string())
            .collect()
    }

    #[test]
    fn accepts_links_that_resolve() {
        let findings = run(&[
            (
                "README.md",
                "# Shikumi\n[licence](LICENSE) [bench](bench/) [ws](bench)\n[top](#shikumi) [abs](/bench/Cargo.toml)\n",
            ),
            ("LICENSE", "text"),
            ("bench/Cargo.toml", ""),
            (
                "docs/guide.md",
                "## Two Words, punctuated!\n## Two Words, punctuated!\n<a id=\"custom\"></a>\n[up](../README.md#shikumi) [dup](#two-words-punctuated-1) [html](#custom)\n[web](https://example.com/x.md) [mail](mailto:a@b.c) <a@b.c> [space](my%20file.md)\n",
            ),
            (
                "docs/my file.md",
                "`code` in [ref][r]\n\n[r]: guide.md#two-words-punctuated\n",
            ),
        ]);
        assert!(findings.is_empty(), "{findings:#?}");
    }

    #[test]
    fn fails_on_a_planted_broken_link() {
        let findings = run(&[
            (
                "README.md",
                "# Title\n\nsee [the guide](docs/missing.md) for more\n",
            ),
            ("docs/real.md", "x"),
        ]);
        assert_eq!(
            findings,
            ["README.md:3:5: docs/missing.md doesn't reach a tracked path"]
        );
    }

    #[test]
    fn fails_on_each_kind_of_breakage() {
        let findings = run(&[
            (
                "a.md",
                "# A\n[x](b.md#nowhere)\n[y](../../etc/passwd)\n[z][undefined]\n[w](a.md/)\n",
            ),
            ("b.md", "# B\n"),
        ]);
        assert_eq!(findings.len(), 4, "{findings:#?}");
        assert!(findings[0].contains("no anchor #nowhere"));
        assert!(findings[1].contains("leaves the repository"));
        assert!(findings[2].contains("[undefined] has no definition"));
        assert!(findings[3].contains("a.md/ doesn't reach"));
    }

    #[test]
    fn ignores_links_inside_code() {
        let findings = run(&[("a.md", "`[x](missing.md)`\n\n```\n[y](missing.md)\n```\n")]);
        assert!(findings.is_empty(), "{findings:#?}");
    }

    #[test]
    fn passes_a_tree_with_no_markdown_pages_beyond_the_root() {
        assert!(run(&[("README.md", "# Only\n"), ("src/lib.rs", "")]).is_empty());
    }
}
