# Working as an agent

These rules cover how an agent works in this repository. Everything in
[contributing](../contributing.md) applies too. This page adds what only matters to an agent, because
an agent knows only the intent that's written down where it looks.

## Find the rules before you change anything

Read the root AGENTS.md, then the AGENTS.md nearest the files you're about to change, if there is one.
The nearest file wins when two disagree. Read [the philosophy](../philosophy.md) before a change that
adds a dependency, a contract or a component.

Don't count on your tool to load these files for you. Some tools load every AGENTS.md from the
repository root down to where they started and none below it; others load upward from where they
started and a deeper one only after opening a file there. Some concatenate what they load and settle a
contradiction either way. The precedence is a rule you apply, not one the tool applies.

## Stay inside your assignment

Change only what you were asked to change. Leave work you didn't make alone, including uncommitted
files, other worktrees, the stash and other sessions' scratch. Don't format it, fix it or revert it,
even when it looks wrong. Mention it in your report instead.

Stage files by name, never with `git add -A`: another session may have changes in the same checkout.
Don't commit, amend, rebase or push unless you were asked to.

## Report conflicts upward and wait

When you hit a decision that isn't yours, stop and report it to whoever gave you the work. That
covers:

- two rules that disagree
- a protected invariant your change would break
- a contract another component reads that your change would alter
- a missing capability you'd have to invent to continue
- the same gate failing twice for the same reason after a fix aimed at that reason

Give the evidence and your recommendation, then hold. Don't pick a side yourself, and don't work
around the conflict.

## Read before you claim

Don't state a fact about this repository unless you read it in the file. Pinned versions run ahead of
what you remember, so read a crate's source at its pinned version, or version-accurate docs, before
relying on an API. Web search never settles a question about this repository or a pinned API.

## Use the dev shell

Run toolchain commands inside `nix develop`, as [contributing](../contributing.md#the-shell) says.
Don't call a tool by a store path you found somewhere; if a tool is missing, the shell is what needs
changing, and that's a change to report, not to work around.

## Gates and long runs

A full gate run is a long run, as [the root AGENTS.md](../../AGENTS.md#gates) says. Ask before
starting one unless your assignment already allows it, and run it through whatever queue the person
you work for names. Send long output to a file under `.runtime/` and read findings from it.

A red gate is a result. Report it with its log and your diagnosis; don't rerun it hoping for green.
A failure that comes and goes is a defect showing itself, and a run that didn't fail proves nothing.

## Leave nothing behind

Scratch goes in `.runtime/<task>/`, as [contributing](../contributing.md#scratch) describes. Before you
report, delete the disposable scratch you created, by its literal path, and list what you kept as
evidence and where it is.
