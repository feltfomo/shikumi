//! running CLI invocations: one process each, killed if its caller is dropped, with arguments
//! split into batches that fit one command line.

use std::io;
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};
use tokio::process::{Child, ChildStdout, Command};

use crate::TransportError;

/// how many argument bytes one invocation carries. Linux allows far more, but a batch this size
/// already amortizes the process start, and staying low keeps clear of `ARG_MAX` on macOS.
const ARGUMENT_BYTES: usize = 128 * 1024;

/// runs a command to completion and returns its stdout, or its stderr in the error when it fails.
pub(crate) async fn run(
    program: &str,
    args: &[String],
    stdin: Option<Vec<u8>>,
) -> Result<Vec<u8>, TransportError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| TransportError::Spawn {
            program: program.to_owned(),
            source,
        })?;
    // stdin is written while the output is read, so a child that answers before it has read
    // everything can't deadlock against a full pipe. the pipe closes when the write finishes.
    let pipe = child.stdin.take();
    let feed = async move {
        match (pipe, stdin) {
            (Some(mut pipe), Some(bytes)) => pipe.write_all(&bytes).await,
            _ => Ok(()),
        }
    };
    let (fed, output) = tokio::join!(feed, child.wait_with_output());
    let output = output.map_err(|source| TransportError::Spawn {
        program: program.to_owned(),
        source,
    })?;
    if !output.status.success() {
        return Err(TransportError::Failed {
            command: describe(program, args),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    fed.map_err(|source| TransportError::Spawn {
        program: program.to_owned(),
        source,
    })?;
    Ok(output.stdout)
}

/// starts a command whose stdout is read as it's produced. the process is killed if the reader is
/// dropped before it ends, and its stderr goes to this process's, so a failure's own message
/// reaches whoever runs it.
pub(crate) fn stream(program: &str, args: &[String]) -> io::Result<Streamed> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    Ok(Streamed {
        _child: child,
        stdout,
    })
}

pub(crate) struct Streamed {
    _child: Child,
    stdout: ChildStdout,
}

impl AsyncRead for Streamed {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stdout).poll_read(cx, buf)
    }
}

/// a command line short enough for an error message.
pub(crate) fn describe(program: &str, args: &[String]) -> String {
    const SHOWN: usize = 4;
    let mut out = program.to_owned();
    for arg in args.iter().take(SHOWN) {
        out.push(' ');
        out.push_str(arg);
    }
    if args.len() > SHOWN {
        out.push_str(&format!(" ... ({} more)", args.len() - SHOWN));
    }
    out
}

/// splits items into batches whose arguments stay under the per-invocation budget. an item larger
/// than the budget gets a batch of its own rather than being refused.
pub(crate) fn batches<T>(items: Vec<T>, arg_len: impl Fn(&T) -> usize) -> Vec<Vec<T>> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    let mut bytes = 0;
    for item in items {
        let len = arg_len(&item) + 1;
        if !current.is_empty() && bytes + len > ARGUMENT_BYTES {
            out.push(std::mem::take(&mut current));
            bytes = 0;
        }
        bytes += len;
        current.push(item);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_split_at_the_byte_budget_and_keep_order() {
        let items: Vec<usize> = (0..10).collect();
        let out = batches(items, |_| ARGUMENT_BYTES / 4);
        assert_eq!(
            out,
            vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7, 8], vec![9]]
        );
    }

    #[test]
    fn an_oversized_item_gets_its_own_batch() {
        let out = batches(vec![1, 2], |_| ARGUMENT_BYTES * 2);
        assert_eq!(out, vec![vec![1], vec![2]]);
    }

    #[test]
    fn a_failing_command_reports_its_stderr() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let args = vec!["-c".to_owned(), "echo nope >&2; exit 3".to_owned()];
        match runtime.block_on(run("sh", &args, None)) {
            Err(TransportError::Failed { stderr, .. }) => assert_eq!(stderr, "nope\n"),
            other => panic!("expected a failure carrying stderr, got {other:?}"),
        }
    }

    #[test]
    fn stdin_reaches_the_command() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = runtime
            .block_on(run("cat", &[], Some(b"through".to_vec())))
            .unwrap();
        assert_eq!(out, b"through");
    }
}
