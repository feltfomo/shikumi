# Architecture pages

How to write and keep an architecture page, such as Shikumi's
[`explanation/architecture.md`](../explanation/architecture.md). Read [the writing guide](../writing.md)
first; its prose rules apply here.

An architecture page is for an engineer who is about to change the system, or who needs to know where
a change belongs. It answers what the system is for, who owns each part, how work moves through it,
how long each thing lives, where it may be extended, which invariants protect it, what it depends on
and how it fails. It's the map. Exact semantics live in the contracts it links to.

## What a page covers

Purpose comes first, in a paragraph: the outcome the system exists to protect and the line it draws
against its neighbours. Name what it doesn't own as plainly as what it does. For Shikumi that line
runs between Shikumi and the Nix daemon, the compilers and the consuming repository.

Ownership names real components, crates, directories and files, with the concern each one owns. One
concern has one owner. If two places decide the same thing, the page says which one is the authority
and treats the other as a defect to fix, not a second rule to describe.

Core flow is the path a normal piece of work takes, start to finish, and where each step runs: which
process, which thread or pool, inside the Nix sandbox or outside it, on the evaluator or in the core.
Execution placement is part of the architecture. A page that says "the evaluator isn't in the edit
loop" while the code calls it on every edit is wrong in a way that costs someone a measurement.

Lifetimes say what is created and destroyed together: a process, a command, a build, a worktree, a
service, a store path and its GC root. Say which lifetime holds which resource and what ends it.

Extension points are the seams a new language backend, store transport, lease provider or repository
extension plugs into, with the contract that governs each. Registering an extension in one central
place is ordinary integration. What signals a wrong seam is one extension's policy duplicated into
the core.

Protected invariants are the rules a change must not break. State each as a rule a reviewer can check,
and link the contract or test that holds it. Keep four things apart: what the code does today, the
invariants it's required to hold, known places where it doesn't hold one yet, and improvements
accepted for later. Code that breaks an established invariant is a defect, recorded as a known
violation with its owner. It doesn't demote the invariant to a future target. Only a guarantee the
system never claimed, a stronger rule not yet adopted, is written as a target.

Dependencies are the allowed directions between parts and the outside systems the system relies on,
with the pins that matter. The repository's declared rules, such as the root AGENTS.md's protected
invariants, are the authority; a page describes the shape those rules produce and adds nothing that
contradicts them.

Failure behavior says what happens when each stage can't finish: what degrades, what's recorded,
what's refused, what stops the whole operation, and what the system never does on failure.

## Shape

There's no template. A page writes the sections it needs, in the order its reader needs them, and one
concern can take a page while another takes a sentence. Don't impose a named industry pattern on a
system that wasn't built as one, and don't describe the system in a vocabulary its code doesn't use.

Use a diagram where it explains a real flow or dependency direction that prose would make the reader
assemble in their head. Draw it in Mermaid, as [the writing guide](../writing.md#show-before-you-tell)
says. Every box is something a reader can open, and every edge is a call, a dependency or a handoff
the code makes. A diagram of intended structure is labelled as a target. Don't draw a diagram for
symmetry with another page.

## Open decisions

A mechanism still being chosen is marked **open**. The page says what's open, the candidates, what
every candidate has to satisfy and what decides between them, such as a measurement on the benchmark
fixtures. It doesn't describe one candidate as if it had won. When the decision lands, the chosen
mechanism replaces the open section in the same change, and the rejected candidates leave the page.

## What stays out

[Placing docs](../placing.md) says where each kind of material goes. The page names a contract, says
why it's there and links its schema; copying a contract's fields or numbers into the page makes a
second authority that drifts. Capability, support and limitation statements go on
[the capability page](capability-pages.md), measured results in records, and procedures in how-to
guides. Plans and the order work will happen in live outside the repository. Design history belongs
in Git, except where the live structure reads wrong without it; then one or two sentences naming the
event, as the writing guide's history rule allows.

## Keeping it true

[Maintaining docs](../maintaining.md#when-code-changes) says which changes update the page. A reviewer
who finds the code and the page disagreeing treats it as a defect in whichever is wrong, and source
wins until someone decides otherwise.

The architecture can evolve. An owner can split, a flow can move to another process, an invariant can
be replaced by a better one. That happens as an explicit, reviewed change that states the old shape,
the new shape and why, and lands with its contracts and tests. It never happens by a feature quietly
working around the old rule.

Difficulty doesn't license a bypass. If honouring an invariant makes a fix hard, the fix is harder; it
doesn't get to break the invariant locally, add a special case beside the owner or leave the page
describing a rule the code no longer keeps. When the invariant itself is wrong, say so and change it
through review.

Mark each statement's status when it isn't current behavior. A page describes the tree as it is, and
an accepted target or an open decision is labelled as one, using the vocabulary in
[the capability-pages guide](capability-pages.md#status). A target that lands moves into the current
description in the same change, and one that's dropped is removed.
