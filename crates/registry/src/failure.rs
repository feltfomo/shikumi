//! reading an evaluation failure out of the evaluator's error. this is the one place that parses an
//! evaluator's text, which isn't a contract Shikumi controls, so it reads only what CppNix,
//! Determinate Nix and Lix print alike under `--show-trace`, and the `registry-no-builds-*` checks
//! hold every kind it names to one classification on each of them.
//!
//! every implementation prints `error:`, then one frame per line that starts with `… `, outermost
//! first, each followed by `at FILE:LINE:COLUMN:` where it has a position and a snippet of the
//! source, and finally `error: MESSAGE`, with the failure's own position after it where it has one.
//! the frames are split by file: a frame in the repository is the user's, a frame in the declaration
//! layer is Shikumi's, and any other frame, such as one in nixpkgs' `lib`, is neither's to fix and
//! is dropped.
//!
//! the kind is read from the evaluator's message only where a user can't have written it: a thrown
//! error is recognized by its frame before anything is read from its message, which is the user's
//! own text. a message this module doesn't recognize keeps its kind unrecognized and its text,
//! rather than being guessed at.

use std::num::NonZeroU32;

use serde::Deserialize;

use crate::contract::RelativePath;
use crate::declarations::{
    At, Failure, FailureKind, FieldAt, InternalFrame, Origin, ShikumiOrigin, UserOrigin,
};
use crate::document::Segment;
use crate::layer::FRAMES;

/// what a frame of the layer's says after its fixed text and a space, the JSON it carries.
fn framed<'d>(description: &'d str, frame: &str) -> Option<&'d str> {
    description.strip_prefix(frame)?.strip_prefix(' ')
}

/// the line Lix prints after a failure's message and snippet when a derivation was involved.
const DERIVATIONS_NOTE: &str = "note: trace involved the following derivations:";

/// how a frame starts, after the indentation every implementation gives it.
const FRAME: &str = "… ";

/// the frame a `throw` adds: CppNix and Determinate Nix name the builtin, Lix names the cause.
const THROW_FRAMES: [&str; 2] = [
    "while calling the 'throw' builtin",
    "caused by explicit throw",
];

/// the start of the message `abort` gives, before the text that was aborted with.
const ABORT: &str = "evaluation aborted with the following error message: '";

/// the start of each message an operation on a value of the wrong type gives: `expected a` covers
/// "expected an integer" too.
const TYPE_MESSAGES: [&str; 5] = [
    "attempt to call something which is not a function but ",
    "expected a",
    "cannot add ",
    "cannot coerce ",
    "cannot compare ",
];

/// the code nlohmann's JSON writer, which every implementation uses, gives a string that isn't
/// UTF-8, whether it stops at a byte that can't start a character ("invalid UTF-8 byte") or at a
/// character the string ends in the middle of ("incomplete UTF-8 string"), and whatever each
/// implementation puts before it.
const INVALID_UTF8: &str = "[json.exception.type_error.316] ";

/// where the evaluated files lie, so each frame can be told apart, and taken out of a message:
/// the repository's root, the declaration layer's directory and the nixpkgs Shikumi runs on, each
/// as the evaluator prints it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Places<'a> {
    pub(crate) root: &'a str,
    pub(crate) layer: &'a str,
    pub(crate) nixpkgs: &'a str,
}

/// a failure as read from an evaluator's error, and the project whose declaration it happened in,
/// when a frame names one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Classified {
    pub(crate) project: Option<String>,
    pub(crate) failure: Failure,
}

/// reads the error an evaluation that exited unsuccessfully printed on stderr.
pub(crate) fn classify(stderr: &str, places: Places<'_>) -> Classified {
    let Some(error) = Error::parse(stderr) else {
        let printed = stderr.trim();
        return Classified {
            project: None,
            failure: Failure {
                kind: FailureKind::Crashed,
                message: relative(
                    if printed.is_empty() {
                        "the evaluator printed nothing"
                    } else {
                        printed
                    },
                    places,
                ),
                origin: Origin::User(UserOrigin {
                    frames: Vec::new(),
                    field: None,
                }),
            },
        };
    };
    let project = error
        .frames
        .iter()
        .find_map(|frame| framed(frame.description, FRAMES.project))
        .and_then(|name| serde_json::from_str::<String>(name).ok());
    let field = error
        .frames
        .iter()
        .rev()
        .find_map(|frame| framed(frame.description, FRAMES.field))
        .and_then(located);

    // frames inside the innermost forcing of a user's value ran that value's own code, and any of
    // Shikumi's code it called shows up there as a frame of its own. the failure's own position
    // inside such a forcing isn't one: an evaluator can place infinite recursion where the value
    // was used again, as Determinate Nix places it in `serialize.nix`.
    let forcing = error
        .frames
        .iter()
        .rposition(|frame| frame.description == FRAMES.value);
    let mut frames: Vec<At> = Vec::new();
    let mut internal: Vec<InternalFrame> = Vec::new();
    let mut shikumi_ran = false;
    let positions = error
        .frames
        .iter()
        .enumerate()
        .filter_map(|(index, frame)| Some((Placed::Frame(index), frame.position.as_ref()?)))
        .chain(error.position.as_ref().map(|at| (Placed::Error, at)));
    for (placed, position) in positions.rev() {
        match place(position, places) {
            Some(Owner::User(at)) if frames.last() != Some(&at) => frames.push(at),
            Some(Owner::Shikumi(frame)) => {
                shikumi_ran |= frame.file != FRAMES.forced_file
                    && match (placed, forcing) {
                        (_, None) => true,
                        (Placed::Frame(index), Some(forcing)) => index > forcing,
                        (Placed::Error, Some(_)) => false,
                    };
                if internal.last() != Some(&frame) {
                    internal.push(frame);
                }
            }
            _ => {}
        }
    }

    let (kind, message) = kind(&error);
    // a failure is Shikumi's only when it never passed through the user's files and Shikumi's own
    // code ran where it happened: outside every forcing of a user's value, or called from inside
    // one. text that isn't valid UTF-8 is the user's data, which Shikumi's `toJSON` only rejects.
    // a failure with no frame anywhere, like the evaluator running out of stack, says nothing
    // either way and isn't called Shikumi's. Shikumi's frames are kept only for a bug's report, so
    // nothing else changes with Shikumi's source.
    let origin = if frames.is_empty() && shikumi_ran && kind != FailureKind::InvalidUtf8 {
        Origin::Shikumi(ShikumiOrigin { frames: internal })
    } else {
        Origin::User(UserOrigin { frames, field })
    };
    Classified {
        project,
        failure: Failure {
            kind,
            message: relative(&message, places),
            origin,
        },
    }
}

/// where a position was printed: on one of the error's frames, by its index, outermost first, or
/// after the error's message as the failure's own.
#[derive(Clone, Copy)]
enum Placed {
    Frame(usize),
    Error,
}

/// what an error's frames and message say the failure was, and the part of the message that says
/// it for that kind.
fn kind(error: &Error<'_>) -> (FailureKind, String) {
    let message = error.message.as_str();
    let innermost = error.frames.last().map(|frame| frame.description);
    if innermost.is_some_and(|description| THROW_FRAMES.contains(&description)) {
        return (FailureKind::Thrown, message.to_owned());
    }
    if let Some(aborted) = message
        .strip_prefix(ABORT)
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return (FailureKind::Thrown, aborted.to_owned());
    }
    // the failures the evaluator places at a position of its own, which a message alone can't
    // fake into the error's last line.
    if error.position.is_some() {
        if message.starts_with("syntax error, ") {
            return (FailureKind::Syntax, message.to_owned());
        }
        if let Some(name) = message
            .strip_prefix("undefined variable '")
            .and_then(|rest| rest.strip_suffix('\''))
        {
            return (FailureKind::UndefinedVariable, name.to_owned());
        }
        if message == "assertion failed"
            || (message.starts_with("assertion '") && message.ends_with("' failed"))
        {
            return (FailureKind::Assertion, message.to_owned());
        }
    }
    if TYPE_MESSAGES.iter().any(|start| message.starts_with(start)) {
        return (FailureKind::Type, message.to_owned());
    }
    if message.contains(INVALID_UTF8) {
        return (FailureKind::InvalidUtf8, message.to_owned());
    }
    (FailureKind::Unrecognized, message.to_owned())
}

/// the evaluator's text with the repository's location taken out, so the document it reaches is
/// the same from any checkout, and with where Shikumi and its nixpkgs are installed named rather
/// than spelled out.
fn relative(text: &str, places: Places<'_>) -> String {
    text.replace(&format!("{}/", places.root), "")
        .replace(places.layer, "«shikumi»")
        .replace(places.nixpkgs, "«nixpkgs»")
}

enum Owner {
    User(At),
    Shikumi(InternalFrame),
}

fn place(position: &Position<'_>, places: Places<'_>) -> Option<Owner> {
    let line = NonZeroU32::new(position.line)?;
    let column = NonZeroU32::new(position.column)?;
    if let Some(file) = inside(position.file, places.root) {
        let file = RelativePath::new(file).ok()?;
        return Some(Owner::User(At { file, line, column }));
    }
    inside(position.file, places.layer).map(|file| {
        Owner::Shikumi(InternalFrame {
            file: file.to_owned(),
            line,
            column,
        })
    })
}

fn inside<'f>(file: &'f str, directory: &str) -> Option<&'f str> {
    file.strip_prefix(directory)?
        .strip_prefix('/')
        .filter(|rest| !rest.is_empty())
}

// the field frame's JSON: the path the walk took and the nearest position on it.
fn located(json: &str) -> Option<FieldAt> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Written {
        path: Vec<Segment>,
        at: Option<At>,
    }
    let Written { path, at } = serde_json::from_str(json).ok()?;
    Some(FieldAt { path, at })
}

/// an error as printed: its frames, outermost first, its message and its own position.
struct Error<'s> {
    frames: Vec<Frame<'s>>,
    message: String,
    position: Option<Position<'s>>,
}

struct Frame<'s> {
    description: &'s str,
    position: Option<Position<'s>>,
}

struct Position<'s> {
    file: &'s str,
    line: u32,
    column: u32,
}

impl<'s> Error<'s> {
    fn parse(stderr: &'s str) -> Option<Self> {
        // anything before the error, such as a warning or the user's own `builtins.trace`, starts
        // with its own word.
        let mut lines = stderr
            .lines()
            .skip_while(|line| *line != "error:" && !line.starts_with("error: "));
        let first = lines.next()?;
        let mut frames = Vec::new();
        let mut message: Option<Vec<&str>> = first.strip_prefix("error: ").map(|m| vec![m]);
        let mut position = None;
        for line in lines {
            let text = line.trim_start();
            if let Some(lines) = message.as_mut() {
                // the message, blank lines included, runs until its position, its snippet or a
                // note after it.
                if position.is_some() || is_snippet(text) || text == DERIVATIONS_NOTE {
                    break;
                }
                if let Some(at) = parse_position(text) {
                    position = Some(at);
                    continue;
                }
                lines.push(text);
            } else if let Some(description) = text.strip_prefix(FRAME) {
                frames.push(Frame {
                    description,
                    position: None,
                });
            } else if let Some(start) = text.strip_prefix("error: ") {
                message = Some(vec![start]);
            } else if let Some(at) = parse_position(text)
                && let Some(frame) = frames.last_mut()
                && frame.position.is_none()
            {
                frame.position = Some(at);
            }
        }
        let message = message?.join("\n");
        Some(Self {
            frames,
            message: message.trim_end().to_owned(),
            position,
        })
    }
}

// a line of a source snippet: an optional line number, then the gutter.
fn is_snippet(text: &str) -> bool {
    text.trim_start_matches(|c: char| c.is_ascii_digit())
        .starts_with('|')
}

// `at FILE:LINE:COLUMN:`, which Determinate Nix prints without the last colon when it has no
// snippet to show. the file is everything before the last two numbers, so a file name may hold
// colons itself.
fn parse_position(text: &str) -> Option<Position<'_>> {
    let rest = text.strip_prefix("at ")?;
    let rest = rest.strip_suffix(':').unwrap_or(rest);
    let (rest, column) = rest.rsplit_once(':')?;
    let (file, line) = rest.rsplit_once(':')?;
    Some(Position {
        file,
        line: line.parse().ok()?,
        column: column.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLACES: Places<'static> = Places {
        root: "/repo",
        layer: "/layer",
        nixpkgs: "/nixpkgs",
    };

    // the user's frames of a failure that's theirs.
    fn frames(failure: &Failure) -> &[At] {
        match &failure.origin {
            Origin::User(origin) => &origin.frames,
            Origin::Shikumi(_) => panic!("a Shikumi bug, not the user's: {failure:?}"),
        }
    }

    // Shikumi's frames of a failure that's a bug in Shikumi.
    fn internal(failure: &Failure) -> Vec<(&str, u32)> {
        match &failure.origin {
            Origin::Shikumi(origin) => origin
                .frames
                .iter()
                .map(|frame| (frame.file.as_str(), frame.line.get()))
                .collect(),
            Origin::User(_) => panic!("the user's, not a Shikumi bug: {failure:?}"),
        }
    }

    fn is_bug(failure: &Failure) -> bool {
        matches!(failure.origin, Origin::Shikumi(_))
    }

    fn at(file: &str, line: u32, column: u32) -> At {
        At {
            file: RelativePath::new(file).unwrap(),
            line: NonZeroU32::new(line).unwrap(),
            column: NonZeroU32::new(column).unwrap(),
        }
    }

    // an error as every implementation prints it under `--show-trace`, with the frames given and
    // the message last.
    fn printed(frames: &[(&str, Option<&str>)], message: &str, position: Option<&str>) -> String {
        let mut text =
            String::from("warning: something unrelated\ntrace: a user's trace\nerror:\n");
        for (description, place) in frames {
            text.push_str(&format!("       … {description}\n"));
            if let Some(place) = place {
                text.push_str(&format!(
                    "         at {place}:\n            1| x\n             | ^\n"
                ));
            }
            text.push('\n');
        }
        text.push_str(&format!("       error: {message}\n"));
        if let Some(place) = position {
            text.push_str(&format!(
                "       at {place}:\n            5| y\n             | ^\n"
            ));
        }
        text
    }

    #[test]
    fn a_throw_is_known_by_its_frame_on_every_implementation() {
        for frame in THROW_FRAMES {
            let read = classify(
                &printed(
                    &[
                        ("shikumi-project \"app\"", None),
                        ("while calling 'pick'", Some("/repo/app/helpers.nix:2:10")),
                        (frame, Some("/repo/app/helpers.nix:2:39")),
                    ],
                    "syntax error, but thrown",
                    None,
                ),
                PLACES,
            );
            assert_eq!(read.project.as_deref(), Some("app"));
            assert_eq!(read.failure.kind, FailureKind::Thrown, "{frame}");
            assert_eq!(read.failure.message, "syntax error, but thrown");
            assert_eq!(
                frames(&read.failure),
                [at("app/helpers.nix", 2, 39), at("app/helpers.nix", 2, 10)]
            );
        }
    }

    #[test]
    fn each_kind_is_read_from_the_message_the_evaluator_places() {
        let cases = [
            (
                "syntax error, unexpected '=', expecting ';'",
                FailureKind::Syntax,
                "syntax error, unexpected '=', expecting ';'",
            ),
            (
                "undefined variable 'sumary'",
                FailureKind::UndefinedVariable,
                "sumary",
            ),
            (
                "assertion '(__lessThan 0 count)' failed",
                FailureKind::Assertion,
                "assertion '(__lessThan 0 count)' failed",
            ),
            (
                "assertion failed",
                FailureKind::Assertion,
                "assertion failed",
            ),
            (
                "attempt to call something which is not a function but a string: \"x\"",
                FailureKind::Type,
                "attempt to call something which is not a function but a string: \"x\"",
            ),
            (
                "evaluation aborted with the following error message: 'stop here'",
                FailureKind::Thrown,
                "stop here",
            ),
            (
                "JSON processing error: [json.exception.type_error.316] invalid UTF-8 byte at index 4: 0xFF",
                FailureKind::InvalidUtf8,
                "JSON processing error: [json.exception.type_error.316] invalid UTF-8 byte at index 4: 0xFF",
            ),
            (
                "JSON serialization error: [json.exception.type_error.316] incomplete UTF-8 string; last byte: 0xC3",
                FailureKind::InvalidUtf8,
                "JSON serialization error: [json.exception.type_error.316] incomplete UTF-8 string; last byte: 0xC3",
            ),
            (
                "expected an integer but found a string: \"a\"",
                FailureKind::Type,
                "expected an integer but found a string: \"a\"",
            ),
            (
                "cannot add a string to an integer",
                FailureKind::Type,
                "cannot add a string to an integer",
            ),
            (
                "infinite recursion encountered",
                FailureKind::Unrecognized,
                "infinite recursion encountered",
            ),
        ];
        for (message, kind, kept) in cases {
            let read = classify(
                &printed(&[], message, Some("/repo/app/project.nix:5:17")),
                PLACES,
            );
            assert_eq!(
                (read.failure.kind, read.failure.message.as_str()),
                (kind, kept)
            );
            assert_eq!(frames(&read.failure), [at("app/project.nix", 5, 17)]);
        }
    }

    #[test]
    fn a_message_without_the_evaluators_own_position_is_not_taken_for_a_placed_kind() {
        // the text a user could throw or a library could return, with no position of the
        // evaluator's after it.
        for message in [
            "syntax error, unexpected '='",
            "undefined variable 'x'",
            "assertion failed",
        ] {
            let read = classify(&printed(&[], message, None), PLACES);
            assert_eq!(read.failure.kind, FailureKind::Unrecognized, "{message}");
        }
    }

    #[test]
    fn frames_split_into_the_users_and_shikumis_and_others_are_dropped() {
        let read = classify(
            &printed(
                &[
                    (
                        "while evaluating attribute 'projects'",
                        Some("/layer/compose.nix:134:5"),
                    ),
                    ("shikumi-project \"app\"", None),
                    (
                        "while evaluating attribute 'value'",
                        Some("/nix/store/x-source/lib/attrsets.nix:1343:9"),
                    ),
                    ("from call site", Some("/repo/app/project.nix:7:15")),
                    ("while calling 'size'", Some("/layer/serialize.nix:45:19")),
                    ("while calling 'size'", Some("/layer/serialize.nix:45:19")),
                ],
                "'builtins.head' called on an empty list",
                None,
            ),
            PLACES,
        );
        assert_eq!(frames(&read.failure), [at("app/project.nix", 7, 15)]);
        assert!(!is_bug(&read.failure));
    }

    // the user's value fails while Shikumi forces it, in nixpkgs' `lib` or with no position at all,
    // so the only positions left are Shikumi's own, where it forced the value.
    #[test]
    fn a_failure_inside_the_forcing_of_a_users_value_is_the_users() {
        for (inner, message) in [
            (
                vec![(
                    "while calling the 'throw' builtin",
                    Some("/nixpkgs/lib/strings.nix:9:3"),
                )],
                "toInt: Could not convert \"x\" to int.",
            ),
            (Vec::new(), "expected a set but found an integer: 1"),
            (
                vec![(
                    "while calling anonymous lambda",
                    Some("/layer/forced.nix:7:1"),
                )],
                "infinite recursion encountered",
            ),
        ] {
            let mut frames = vec![
                ("shikumi-project \"app\"", None),
                ("while calling 'size'", Some("/layer/serialize.nix:45:19")),
                ("shikumi-value", None),
            ];
            frames.extend(inner);
            let read = classify(
                &printed(&frames, message, Some("/layer/serialize.nix:61:8")),
                PLACES,
            );
            assert!(!is_bug(&read.failure), "{message}: {read:?}");
        }
    }

    // a user's value that calls into Shikumi's Nix, and fails there, is Shikumi's to fix.
    #[test]
    fn shikumis_own_code_failing_inside_a_users_value_is_a_shikumi_bug() {
        let read = classify(
            &printed(
                &[
                    ("shikumi-project \"app\"", None),
                    ("shikumi-value", None),
                    ("while calling 'project'", Some("/layer/factory.nix:6:13")),
                ],
                "expected a set but found null: null",
                None,
            ),
            PLACES,
        );
        assert!(is_bug(&read.failure));
    }

    #[test]
    fn a_message_keeps_its_blank_lines_and_an_abort_its_text() {
        let read = classify(
            "error:\n       … shikumi-project \"app\"\n\n       error: evaluation aborted with the following error message: 'first line\n\n       second line'\n",
            PLACES,
        );
        assert_eq!(read.failure.kind, FailureKind::Thrown);
        assert_eq!(read.failure.message, "first line\n\nsecond line");
    }

    #[test]
    fn shikumis_and_nixpkgs_locations_are_named_rather_than_spelled_out() {
        let read = classify(
            &printed(
                &[],
                "cannot coerce a function to a string: «lambda project @ /layer/factory.nix:6:13» and /nixpkgs/lib/x.nix",
                None,
            ),
            PLACES,
        );
        assert_eq!(
            read.failure.message,
            "cannot coerce a function to a string: «lambda project @ «shikumi»/factory.nix:6:13» and «nixpkgs»/lib/x.nix"
        );
    }

    #[test]
    fn a_failure_only_inside_shikumis_nix_is_a_shikumi_bug() {
        let read = classify(
            &printed(
                &[
                    ("shikumi-project \"app\"", None),
                    ("while calling 'size'", Some("/layer/serialize.nix:45:19")),
                    ("while calling 'size'", Some("/layer/serialize.nix:45:19")),
                    (
                        "while calling 'map'",
                        Some("/nix/store/x-source/lib/lists.nix:3:3"),
                    ),
                    (
                        "while calling the 'throw' builtin",
                        Some("/layer/serialize.nix:142:7"),
                    ),
                ],
                "shikumi: serialization has no case for a Nix value of kind x",
                None,
            ),
            PLACES,
        );
        assert_eq!(
            internal(&read.failure),
            [("serialize.nix", 142), ("serialize.nix", 45)]
        );
    }

    #[test]
    fn text_that_isnt_utf8_is_the_users_even_with_no_frame_of_theirs() {
        let read = classify(
            &printed(
                &[
                    ("shikumi-project \"app\"", None),
                    (
                        "shikumi-field {\"path\":[\"project\",\"summary\"],\"at\":{\"file\":\"app/project.nix\",\"line\":4,\"column\":5}}",
                        None,
                    ),
                    (
                        "while calling the 'toJSON' builtin",
                        Some("/layer/produce.nix:56:7"),
                    ),
                ],
                "JSON serialization error: [json.exception.type_error.316] invalid UTF-8 byte at index 4: 0xFF",
                None,
            ),
            PLACES,
        );
        assert_eq!(read.failure.kind, FailureKind::InvalidUtf8);
        assert!(!is_bug(&read.failure));
        let field = read.failure.field().unwrap().clone();
        assert_eq!(
            field.path,
            [
                Segment::Field("project".to_owned()),
                Segment::Field("summary".to_owned())
            ]
        );
        assert_eq!(field.at, Some(at("app/project.nix", 4, 5)));
    }

    #[test]
    fn the_outermost_project_frame_names_the_project_and_a_users_copy_does_not() {
        let read = classify(
            &printed(
                &[
                    ("shikumi-project \"app\"", None),
                    (
                        "shikumi-project \"forged\"",
                        Some("/repo/app/project.nix:3:3"),
                    ),
                ],
                "boom",
                None,
            ),
            PLACES,
        );
        assert_eq!(read.project.as_deref(), Some("app"));
    }

    #[test]
    fn the_repositorys_location_is_taken_out_of_the_message() {
        let read = classify(
            &printed(&[], "path '/repo/app/nowhere.nix' does not exist", None),
            PLACES,
        );
        assert_eq!(
            read.failure.message,
            "path 'app/nowhere.nix' does not exist"
        );
    }

    #[test]
    fn an_error_printed_without_frames_is_read_too() {
        let read = classify(
            "error: JSON serialization error: [json.exception.type_error.316] invalid UTF-8 byte at index 4: 0xFF\n",
            PLACES,
        );
        assert_eq!(read.failure.kind, FailureKind::InvalidUtf8);
        assert_eq!(read.project, None);
    }

    #[test]
    fn an_evaluator_that_stops_without_an_error_crashed() {
        let read = classify("Segmentation fault\n", PLACES);
        assert_eq!(read.failure.kind, FailureKind::Crashed);
        assert_eq!(read.failure.message, "Segmentation fault");
        assert!(!is_bug(&read.failure));
    }

    #[test]
    fn a_failure_with_no_frame_anywhere_is_nobodys_bug() {
        // what the evaluator's stack overflow handler prints, outside every trace.
        let read = classify(
            "error: stack overflow (possible infinite recursion)\n",
            PLACES,
        );
        assert_eq!(read.failure.kind, FailureKind::Unrecognized);
        assert!(!is_bug(&read.failure));
    }

    #[test]
    fn a_position_without_its_last_colon_is_read() {
        let read = classify(
            "error:\n       … from call site\n         at /repo/app/project.nix:3:4\n\n       error: boom\n",
            PLACES,
        );
        assert_eq!(frames(&read.failure), [at("app/project.nix", 3, 4)]);
    }

    #[test]
    fn a_message_spans_its_lines_and_a_file_name_may_hold_colons() {
        let read = classify(
            "error:\n       … from call site\n         at /repo/a:b/project.nix:3:4:\n            3| x\n\n       error: first line\n       second line\n",
            PLACES,
        );
        assert_eq!(read.failure.message, "first line\nsecond line");
        assert_eq!(frames(&read.failure), [at("a:b/project.nix", 3, 4)]);
    }
}
