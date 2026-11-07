# Capability pages

How to write and keep a capability page, such as Shikumi's [`reference/capabilities.md`](capabilities.md).
Read [the writing guide](../writing.md) first; its prose rules apply here.
[Placing docs](../placing.md) says where material that isn't a capability goes, and
[maintaining docs](../maintaining.md#when-code-changes) says which changes update the page.

A capability page is for someone deciding whether and how to use Shikumi: an engineer adopting it in a
repository, a reviewer judging a claim, someone reading what it produced. It says what Shikumi
provides, what it accepts and produces, what it guarantees, where it applies, where it stops and how
to use it. It opens with that, in the reader's terms, before it needs any symbol from the
implementation. Symbols and paths come in where the reader has to find the entry point or the
contract.

## Status

Every statement about a capability carries one status, either in a status column or in the heading of
the part it belongs to.

| Status            | Meaning                                                                                        |
| ----------------- | ---------------------------------------------------------------------------------------------- |
| Current           | The tree does this today. Source wins where the page disagrees, and the page is fixed.         |
| Limited, waits on | Part of the acceptance can't be met until a named dependency exists. Names it, and the failure that stands in. |
| Accepted target   | Decided and in scope, not built yet or not finished. States the outcome required.             |
| Open              | An accepted target whose mechanism is still being chosen. Names the candidates and what decides. |
| Proposal          | Written down, not decided. Nothing may depend on it.                                           |
| Out of scope      | Deliberately excluded, so it isn't silently reintroduced. Says why in a sentence.              |

A target describes what has to be true, not a mechanism someone imagined, unless the mechanism is
itself the decision. A mechanism still under study is written as **open**, with the candidates and the
acceptance any choice must meet, never as settled. A status that depends on a platform says so, such
as a target that holds on Linux and stays unverified on macOS until there's a machine to verify it on.

A current behavior that falls short of a stated guarantee is a known defect, written as a limitation
with its owner, not relabelled a target. When a target lands, the same change moves it to current and
deletes the target wording. Unimplemented requirements don't get dropped because they're
unimplemented; they stay as targets until someone with the authority to rule removes them from scope.

A limitation names what it waits on and what Shikumi does until then: an explicit failure or a
declared "not supported", never a stub that pretends to work. When the dependency lands, the
limitation is reopened as work.

The status is the only progress a capability page carries. When or in what order a target will be
built belongs to the plan, outside this repository.

## Summaries and contracts

The page summarizes and links; it doesn't restate a contract's fields or a record's numbers. A summary
that would need updating every time the contract changes is too detailed. Where the page and a
contract describe the same boundary, the contract's schema is the authority for exact behavior and the
page says what that behavior gives the reader.

A long capability page doesn't have to split. Split it when a part has a different reader or a
different owner.
