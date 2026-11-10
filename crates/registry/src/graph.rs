//! loops in the two orderings a declaration states: projects' `dependsOn` and a project's steps'
//! `after`. each is a graph over names, and nothing downstream can order one with a loop in it, so
//! the checker refuses a loop and the registry document's contract does too.
//!
//! cost is linear in nodes plus edges: Tarjan's strongly connected components, iterative so a long
//! chain of dependencies can't overflow the stack, then one breadth-first walk inside each component
//! that holds a loop, to name one loop through it.

/// an edge to node `to`, written at position `at` of its node's own list, so a finding can point
/// at the entry that made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Edge {
    pub(crate) to: usize,
    pub(crate) at: usize,
}

use std::collections::HashMap;

const UNSEEN: usize = usize::MAX;

/// one loop through each strongly connected component that holds one: a node with an edge to
/// itself, or two or more nodes that reach each other. each loop is its nodes in order, each with
/// the edge it leaves by, starting from the component's lowest node; the loops come in the order of
/// their first nodes.
pub(crate) fn cycles(graph: &[Vec<Edge>]) -> Vec<Vec<(usize, Edge)>> {
    let count = graph.len();
    let mut index = vec![UNSEEN; count];
    let mut low = vec![0; count];
    let mut on_stack = vec![false; count];
    let mut component = vec![UNSEEN; count];
    let mut stack = Vec::new();
    let mut components: Vec<Vec<usize>> = Vec::new();
    let mut next = 0;
    for root in 0..count {
        if index[root] != UNSEEN {
            continue;
        }
        // each frame is a node and how many of its edges it has followed.
        let mut frames = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(frame) = frames.last_mut() {
            let node = frame.0;
            if let Some(edge) = graph[node].get(frame.1) {
                frame.1 += 1;
                let target = edge.to;
                if index[target] == UNSEEN {
                    index[target] = next;
                    low[target] = next;
                    next += 1;
                    stack.push(target);
                    on_stack[target] = true;
                    frames.push((target, 0));
                } else if on_stack[target] {
                    low[node] = low[node].min(index[target]);
                }
                continue;
            }
            frames.pop();
            if let Some(&(parent, _)) = frames.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == index[node] {
                let id = components.len();
                let mut members = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component[member] = id;
                    members.push(member);
                    if member == node {
                        break;
                    }
                }
                components.push(members);
            }
        }
    }

    // shared by every walk and reset after each, so naming the loops stays linear in the graph.
    let mut previous: Vec<Option<(usize, Edge)>> = vec![None; count];
    let mut loops = Vec::new();
    for (id, members) in components.iter().enumerate() {
        let Some(&start) = members.iter().min() else {
            continue;
        };
        let looped = members.len() > 1 || graph[start].iter().any(|edge| edge.to == start);
        if looped {
            loops.push(walk(graph, &component, id, start, &mut previous));
        }
    }
    loops.sort_by_key(|steps| steps[0].0);
    loops
}

// the shortest loop from `start` back to itself inside its component, which a strongly connected
// component always has.
fn walk(
    graph: &[Vec<Edge>],
    component: &[usize],
    id: usize,
    start: usize,
    previous: &mut [Option<(usize, Edge)>],
) -> Vec<(usize, Edge)> {
    let mut queue = std::collections::VecDeque::from([start]);
    let mut visited = vec![start];
    let mut closing = None;
    'search: while let Some(node) = queue.pop_front() {
        for &edge in &graph[node] {
            if component[edge.to] != id {
                continue;
            }
            if edge.to == start {
                closing = Some((node, edge));
                break 'search;
            }
            if previous[edge.to].is_none() {
                previous[edge.to] = Some((node, edge));
                visited.push(edge.to);
                queue.push_back(edge.to);
            }
        }
    }
    let mut steps = Vec::new();
    let mut step = closing;
    while let Some((node, edge)) = step {
        steps.push((node, edge));
        step = if node == start { None } else { previous[node] };
    }
    steps.reverse();
    for node in visited {
        previous[node] = None;
    }
    steps
}

/// a node's edges in its declared order, to the names `position` knows. a name it doesn't know is
/// an unknown reference, reported on its own, and orders nothing.
pub(crate) fn edges<'n>(
    names: impl IntoIterator<Item = &'n str>,
    position: &HashMap<&str, usize>,
) -> Vec<Edge> {
    names
        .into_iter()
        .enumerate()
        .filter_map(|(at, name)| position.get(name).map(|&to| Edge { to, at }))
        .collect()
}

/// each name's place in `names`, for [`edges`].
pub(crate) fn positions<'n>(names: impl IntoIterator<Item = &'n str>) -> HashMap<&'n str, usize> {
    names
        .into_iter()
        .enumerate()
        .map(|(position, name)| (name, position))
        .collect()
}

/// a loop as a sentence: "`a` depends on `b`, which depends on `a`", with `relation` the verb.
pub(crate) fn sentence(names: &[&str], relation: &str) -> String {
    let Some((first, rest)) = names.split_first() else {
        return String::new();
    };
    if rest.is_empty() {
        return format!("`{first}` {relation} itself");
    }
    let mut text = format!("`{first}` {relation} `{}`", rest[0]);
    for name in rest[1..].iter().chain([first]) {
        text.push_str(&format!(", which {relation} `{name}`"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[&[usize]]) -> Vec<Vec<Edge>> {
        edges
            .iter()
            .map(|targets| {
                targets
                    .iter()
                    .enumerate()
                    .map(|(at, &to)| Edge { to, at })
                    .collect()
            })
            .collect()
    }

    fn nodes(found: &[Vec<(usize, Edge)>]) -> Vec<Vec<usize>> {
        found
            .iter()
            .map(|steps| steps.iter().map(|(node, _)| *node).collect())
            .collect()
    }

    #[test]
    fn a_graph_without_a_loop_has_none() {
        assert!(cycles(&graph(&[&[1, 2], &[2], &[]])).is_empty());
    }

    #[test]
    fn every_component_with_a_loop_is_named_once_from_its_lowest_node() {
        // 0 -> 1 -> 2 -> 0, 3 -> 3, and 4 -> 5 -> 4 reached from 2.
        let found = cycles(&graph(&[&[1], &[2], &[0, 4], &[3], &[5], &[4]]));
        assert_eq!(nodes(&found), [vec![0, 1, 2], vec![3], vec![4, 5]]);
        assert_eq!(found[0][2].1, Edge { to: 0, at: 0 });
    }

    #[test]
    fn the_loop_named_is_a_shortest_one_through_the_lowest_node() {
        // 0 -> 1 -> 2 -> 3 -> 0 and the shortcut 1 -> 3.
        let found = cycles(&graph(&[&[1], &[2, 3], &[3], &[0]]));
        assert_eq!(nodes(&found), [vec![0, 1, 3]]);
        assert_eq!(found[0][1].1, Edge { to: 3, at: 1 });
    }

    #[test]
    fn a_long_chain_doesnt_exhaust_the_stack() {
        let count = 200_000;
        let mut edges: Vec<Vec<Edge>> = (0..count)
            .map(|node| {
                vec![Edge {
                    to: node + 1,
                    at: 0,
                }]
            })
            .collect();
        edges[count - 1] = vec![Edge { to: 0, at: 0 }];
        let found = cycles(&edges);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].len(), count);
    }

    #[test]
    fn a_loop_reads_as_one_sentence() {
        assert_eq!(sentence(&["a"], "depends on"), "`a` depends on itself");
        assert_eq!(
            sentence(&["a", "b", "c"], "runs after"),
            "`a` runs after `b`, which runs after `c`, which runs after `a`"
        );
    }
}
