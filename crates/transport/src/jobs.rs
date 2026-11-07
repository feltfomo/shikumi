//! bounded concurrent jobs owned by the call that starts them, shared by both transports: CLI
//! invocations and daemon connections alike run through here, so neither keeps its own scheduler.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use tokio::task::JoinSet;

use crate::TransportError;

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

/// which nodes wait for which: the one rule for ordering a batch whose items depend on each other,
/// whether it's scheduled as it runs or sent in one stream. building it and walking it are linear
/// in nodes plus edges.
#[derive(Debug)]
pub(crate) struct Graph {
    waiting: Vec<usize>,
    dependents: Vec<Vec<usize>>,
}

impl Graph {
    /// `waits_for[i]` lists the nodes node `i` waits for.
    pub(crate) fn new(waits_for: &[Vec<usize>]) -> Self {
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); waits_for.len()];
        for (node, inputs) in waits_for.iter().enumerate() {
            for &input in inputs {
                dependents[input].push(node);
            }
        }
        Self {
            waiting: waits_for.iter().map(Vec::len).collect(),
            dependents,
        }
    }

    fn ready(&self) -> VecDeque<usize> {
        (0..self.waiting.len())
            .filter(|&node| self.waiting[node] == 0)
            .collect()
    }

    /// marks `node` done and queues every node that was waiting only for it.
    fn finish(&mut self, node: usize, ready: &mut VecDeque<usize>) {
        for &dependent in &self.dependents[node] {
            self.waiting[dependent] -= 1;
            if self.waiting[dependent] == 0 {
                ready.push_back(dependent);
            }
        }
    }

    /// an order in which every node comes after the nodes it waits for, otherwise in input order.
    pub(crate) fn order(mut self) -> Result<Vec<usize>, TransportError> {
        let mut ready = self.ready();
        let mut order = Vec::with_capacity(self.waiting.len());
        while let Some(node) = ready.pop_front() {
            order.push(node);
            self.finish(node, &mut ready);
        }
        if order.len() == self.waiting.len() {
            Ok(order)
        } else {
            Err(cycle())
        }
    }
}

/// a batch whose items wait on each other in a cycle has no item that can go first. Shikumi's own
/// batches can't hold one, since a path is hashed with what it refers to.
fn cycle() -> TransportError {
    TransportError::Unsupported("the batch's items wait on each other in a cycle".to_owned())
}

/// runs one job per node, starting each as soon as every node it waits for has finished, with at
/// most `limit` in flight, and returns the results in node order. `waits_for[i]` lists the nodes
/// node `i` waits for, and a cycle among them is an error. a node never waits on a node that's
/// merely slow and unrelated, and cost is linear in nodes plus edges.
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
    let mut graph = Graph::new(waits_for);
    let mut ready = graph.ready();
    let mut results: Vec<Option<T>> = (0..waits_for.len()).map(|_| None).collect();
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
        graph.finish(node, &mut ready);
    }
    // a node in a cycle is never ready, so it's the one left without a result.
    results
        .into_iter()
        .map(|result| result.ok_or_else(cycle))
        .collect()
}

/// polls every future in the calling task until all have finished, and returns their results in
/// input order, or the first error at once. nothing is spawned, so the futures may borrow from the
/// caller; each wake polls every unfinished one, which is cheap for the handful a caller splits a
/// batch into.
pub(crate) async fn join_all<T, F>(futures: Vec<F>) -> Result<Vec<T>, TransportError>
where
    F: Future<Output = Result<T, TransportError>>,
{
    let mut pending: Vec<Option<Pin<Box<F>>>> = futures
        .into_iter()
        .map(|future| Some(Box::pin(future)))
        .collect();
    let mut results: Vec<Option<T>> = (0..pending.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut unfinished = false;
        for (slot, result) in pending.iter_mut().zip(&mut results) {
            let Some(future) = slot else {
                continue;
            };
            match future.as_mut().poll(cx) {
                Poll::Ready(Ok(value)) => {
                    *result = Some(value);
                    *slot = None;
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => unfinished = true,
            }
        }
        if unfinished {
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    })
    .await?;
    Ok(results
        .into_iter()
        .map(|result| result.expect("every future finished"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn an_order_puts_each_node_after_what_it_waits_for() {
        let order = Graph::new(&[vec![2], vec![], vec![1]]).order().unwrap();
        assert_eq!(order, vec![1, 2, 0]);
    }

    // both a stream ordered up front and a graph scheduled as it runs refuse a cycle the same way,
    // rather than one of them panicking.
    #[test]
    fn a_cycle_is_an_error_whether_ordered_or_run() {
        let cyclic = [vec![1], vec![0], vec![]];
        assert!(matches!(
            Graph::new(&cyclic).order(),
            Err(TransportError::Unsupported(_))
        ));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let run = runtime.block_on(run_graph(2, &cyclic, |node| async move { Ok(node) }));
        assert!(matches!(run, Err(TransportError::Unsupported(_))));
    }

    #[test]
    fn joined_futures_keep_input_order_and_fail_on_the_first_error() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let futures: Vec<_> = (0..4u64)
            .map(|i| async move {
                tokio::time::sleep(std::time::Duration::from_millis(4 - i)).await;
                Ok(i)
            })
            .collect();
        assert_eq!(
            runtime.block_on(join_all(futures)).unwrap(),
            vec![0, 1, 2, 3]
        );
        let failing = vec![
            Box::pin(std::future::pending::<Result<u64, TransportError>>())
                as Pin<Box<dyn Future<Output = Result<u64, TransportError>>>>,
            Box::pin(async { Err(TransportError::Unsupported("no".to_owned())) }),
        ];
        assert!(runtime.block_on(join_all(failing)).is_err());
    }
}
