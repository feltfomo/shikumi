# Maintaining docs

A doc stays true because the change that makes it stale also updates it. Shikumi has no doc tool, so
two gate checks hold the mechanical part and review holds the rest. This page says which is which,
and what a code change owes its docs.

## What the gate holds

The gate's `links` and `private-names` checks, described in
[the root AGENTS.md's Gates section](../AGENTS.md#gates), are the mechanical part. Two things about
them decide what's left for review.

A fragment link is checked against the target page's headings, slugged the way GitHub slugs them,
and its HTML `id` and `name` anchors. Renaming a heading breaks every link to it, and the check says
where.

`private-names` matches whole tokens, so it catches a protected name written out, in a path or fused
by a case change, and nothing else. A description of a private consumer, its projects or the
maintainer's machines without the name gets past it.

## What review holds

Everything a check can't read:

- whether prose still describes the code
- whether a status on the capability page is still right
- whether a fact has one home, and every other page links to it rather than restating it
- whether a page follows its type's guide and [the writing guide](writing.md)
- whether anything describes a private consumer, the maintainer's machines or a plan

When a doc tool exists, generated facts move from review to the tool, and this page changes with it.

## When code changes

Follow the code change through its docs in the same change.

A change to Shikumi's shape updates [the architecture page](explanation/architecture.md) in the same
change. Shape means an owner added, removed or moved, a flow or where it runs, a lifetime, an
extension point, a dependency direction, an invariant added, weakened or removed, or failure
behavior.

A change to what Shikumi provides updates [the capability page](reference/capabilities.md) in the same
change. That covers:

- a capability added, removed or narrowed
- support changing: a Nix implementation, platform, language or toolchain becoming supported or
  stopping being
- an interface a consumer uses: a command, a flag, a declaration field, an extension slot or a
  contract
- failure behavior a consumer sees: what's refused, what's reported, what degrades
- a capability's status changing, in [the status vocabulary](reference/capability-pages.md#status)

An open mechanism that gets settled leaves the open wording. Its answer goes where it now belongs: the
chosen mechanism on the architecture page, and the capability's status on the capability page.

A contract change bumps the version its schema records and regenerates the tracked schema in the same
change. The pages that summarize the contract follow.

A change to the gate updates the root AGENTS.md's Gates section, its one home. A change to the dev
shell's tools updates [contributing](contributing.md#the-shell).

## Records

Records are history. Once a record is written, it isn't edited, except to fix a broken link. A new
finding gets a new record, which links to the old one.

## Moving or retiring a page

Restructuring is expected, but no requirement gets lost on the way. Before deleting a paragraph, find
where each obligation in it now lives, and if it lives nowhere, keep it. Say where moved obligations
went in the change's description.

When you move a page or rename a heading, fix every link that points to it, in docs, AGENTS.md and
source comments, and delete the old copy in the same change. Retire superseded wording outright
rather than leaving it beside the new; two versions of one rule are worse than either.

Update [the front door](README.md) whenever a page is added, moved, retitled or retired, or its first
sentence changes. Until a tool generates that list, keeping it in step is part of the change.
