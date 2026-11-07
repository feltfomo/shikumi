# Placing docs

Where a doc goes depends on its page type. Shikumi is one project, so there's one tree, and the type
decides the directory.

## The tree

```text
docs/
  README.md          front door: every page by title and first sentence
  philosophy.md      Shikumi's laws
  contributing.md    shell, gate, harness, tests, dependencies, scratch; everyone reads it
  writing.md placing.md maintaining.md
  ai/                agent rules, starting at ai/working.md
  reference/         style guides, page-type guides, the capability page, contract pages
  explanation/       the architecture page and other design explanations
  how-to/            task steps for something built
  tutorials/         a guided first use of something built
  records/           what a measurement, investigation or audit found
```

Create a type directory only when it has a page to hold. `how-to/`, `tutorials/` and `records/` don't
exist yet, because nothing they'd describe is built or measured.

## Which page type

| You're writing...                                        | Type        | Goes in                       |
| -------------------------------------------------------- | ----------- | ----------------------------- |
| what Shikumi provides, with each capability's status     | reference   | `reference/capabilities.md`   |
| a contract's fields and versions, summarizing its schema | reference   | `reference/`                  |
| a style guide for a language, or a page type's guide     | reference   | `reference/`                  |
| how Shikumi is put together, its owners and invariants   | explanation | `explanation/architecture.md` |
| why one part is designed the way it is                   | explanation | `explanation/`                |
| steps to run, add, debug or release something built      | how-to      | `how-to/`                     |
| a guided first use for a newcomer                        | tutorial    | `tutorials/`                  |
| what a benchmark run or investigation found              | record      | `records/`                    |
| how agents should behave while working here              | ai          | `ai/`                         |
| a roadmap, a plan or research toward a decision          | none        | outside this repository       |

There's one capability page and one architecture page. When a part of Shikumi grows enough to need
its own, split by responsibility and keep the original as the map that links to the parts.

A machine contract's authority is its published schema, tracked beside the code that owns it, as
`bench/schema/` is. A reference page about a contract summarizes and links that schema, and never
copies its fields into a second authority.

A capability nobody has built gets its line on the capability page and nothing else: no how-to or
tutorial for a feature that doesn't exist.

## What counts as `ai/`

A doc goes in `ai/` because of what it governs, not because of who reads it. It holds the rules for
how an agent works: staying inside its assignment, reporting a conflict and waiting, and which shell
and commands to use. Anything a human engineer needs too, such as the writing guide or a style guide,
goes in the normal tree, even when agents read it most.

## AGENTS.md

An AGENTS.md isn't a doc. It's the short list of rules for its scope, and it points into `docs/` for
the reasons behind them. When a rule needs more than three lines of explanation, put the explanation
in a doc and keep one line plus a link in AGENTS.md. The root AGENTS.md is the only one today; a
directory gets its own only when its maintainers set rules that bind it alone, and that file links
up to the root.
