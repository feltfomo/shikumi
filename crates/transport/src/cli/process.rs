//! running CLI invocations: one process each, killed if its caller is dropped, and many at once
//! with their results kept in input order.

use std::collections::VecDeque;
use std::future::Future;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::task::JoinSet;

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

/// runs the jobs with at most `limit` in flight and returns their results in input order.
pub(crate) async fn run_all<T, F>(limit: usize, jobs: Vec<F>) -> Result<Vec<T>, TransportError>
where
    T: Send + 'static,
    F: Future<Output = Result<T, TransportError>> + Send + 'static,
{
    let independent = vec![Vec::new(); jobs.len()];
    let mut jobs: Vec<Option<F>> = jobs.into_iter().map(Some).collect();
    run_graph(limit, &independent, |index| {
        jobs[index]
            .take()
            .expect("the scheduler starts each job once")
    })
    .await
}

/// runs one job per node, starting each as soon as every node it waits for has finished, with at
/// most `limit` in flight, and returns the results in node order. `waits_for[i]` lists the nodes
/// node `i` waits for; they must form no cycle. a node never waits on a node that's merely slow
/// and unrelated, and cost is linear in nodes plus edges.
///
/// the first error returns at once; dropping the set aborts the rest, and their processes die with
/// them.
pub(crate) async fn run_graph<T, F, J>(
    limit: usize,
    waits_for: &[Vec<usize>],
    mut job: J,
) -> Result<Vec<T>, TransportError>
where
    T: Send + 'static,
    F: Future<Output = Result<T, TransportError>> + Send + 'static,
    J: FnMut(usize) -> F,
{
    let count = waits_for.len();
    let mut waiting: Vec<usize> = waits_for.iter().map(Vec::len).collect();
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (node, inputs) in waits_for.iter().enumerate() {
        for &input in inputs {
            dependents[input].push(node);
        }
    }
    let mut ready: VecDeque<usize> = (0..count).filter(|&node| waiting[node] == 0).collect();
    let mut results: Vec<Option<T>> = (0..count).map(|_| None).collect();
    let mut running = JoinSet::new();
    loop {
        while running.len() < limit.max(1) {
            let Some(node) = ready.pop_front() else {
                break;
            };
            let future = job(node);
            running.spawn(async move { (node, future.await) });
        }
        let Some(joined) = running.join_next().await else {
            break;
        };
        let (node, result) = match joined {
            Ok(done) => done,
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(error) => unreachable!("transport jobs are never aborted while awaited: {error}"),
        };
        results[node] = Some(result?);
        for &dependent in &dependents[node] {
            waiting[dependent] -= 1;
            if waiting[dependent] == 0 {
                ready.push_back(dependent);
            }
        }
    }
    Ok(results
        .into_iter()
        .map(|result| result.expect("an acyclic graph runs every node"))
        .collect())
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
    fn results_come_back_in_input_order_whatever_finishes_first() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let jobs: Vec<_> = (0..8u64)
            .map(|i| async move {
                tokio::time::sleep(std::time::Duration::from_millis(8 - i)).await;
                Ok(i)
            })
            .collect();
        let out = runtime.block_on(run_all(3, jobs)).unwrap();
        assert_eq!(out, (0..8).collect::<Vec<_>>());
    }

    // node 1 waits for the quick node 0, and node 2 is slow and unrelated. node 1 has to start
    // while node 2 still runs: grouping nodes into waves would hold it until node 2 ends.
    #[test]
    fn a_node_starts_once_its_own_inputs_finish_not_its_whole_level() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let waits_for = vec![vec![], vec![0], vec![]];
        let delays = [1, 1, 50];
        let out = runtime
            .block_on(run_graph(4, &waits_for, |node| {
                let events = events.clone();
                async move {
                    events.lock().unwrap().push(format!("start {node}"));
                    tokio::time::sleep(std::time::Duration::from_millis(delays[node])).await;
                    events.lock().unwrap().push(format!("end {node}"));
                    Ok(node)
                }
            }))
            .unwrap();
        assert_eq!(out, vec![0, 1, 2]);
        let events = events.lock().unwrap();
        let at = |event: &str| events.iter().position(|e| e == event).unwrap();
        assert!(at("end 0") < at("start 1"), "{events:?}");
        assert!(at("start 1") < at("end 2"), "{events:?}");
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
