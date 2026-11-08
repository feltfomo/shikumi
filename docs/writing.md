# Writing docs

This guide covers every doc in `docs/`, whoever the reader is, and the comments in the code.
[Placing docs](placing.md) says where a doc goes, and [maintaining docs](maintaining.md) says what
keeps it true. Two page types have guides of their own: [architecture pages](reference/architecture-pages.md)
and [capability pages](reference/capability-pages.md).

## Write for one reader doing one thing

Before writing, name the reader and what they're trying to do. Each page answers one of four needs,
following [Diátaxis](https://diataxis.fr/):

| The reader wants to... | Page type   | It reads like                                  |
| ---------------------- | ----------- | ---------------------------------------------- |
| learn by doing         | tutorial    | a lesson with a guaranteed result              |
| get a task done        | how-to      | steps, starting from where they already are    |
| look something up      | reference   | exact, complete and terse, the way a table is  |
| understand why         | explanation | prose about design, constraints and trade-offs |

The guides at the top of `docs/`, this one included, are rules for working here and sit outside the
four. So do records, which keep what happened: a benchmark campaign, an investigation, an audit.

Don't mix types on one page. A reference page that stops to justify a design loses the reader who
came to look something up. Put the why in the explanation page and link to it.

## Show before you tell

Open every page with what the reader needs first, in two or three sentences. Next comes whatever
shows the thing fastest: a diagram, a snippet, or for a reference page a table. Only then does the
detail start.

- **Diagrams:** draw one for any structure, flow or ownership split that takes more than a paragraph
  to describe. Use Mermaid in a fenced `mermaid` block, which GitHub renders. Keep a diagram to about
  a dozen nodes, and split it by zoom level when it grows past that.
- **Snippets:** show real usage for any command, interface, file format or config. Copy it from
  something that runs, label the fence with its language, and cut it down to the part that matters.
  A feature that isn't built has no snippet yet; don't invent one.

## Keep it light

Dense docs get skimmed, and skimming misses the line that mattered. These limits are targets, so
break one when the content needs it, not by habit.

- A paragraph makes one point in about five sentences at most.
- A table cell holds a phrase. Anything that needs a full sentence goes below the table.
- A page past roughly 400 lines is probably two page types, so split it.
- Prose names a path only where the reader has to open that file.
- Give every term one name and keep using it. Define a term the reader might not know when it first
  appears.

Use headings for navigation, not for every thought. Most ideas fit in a paragraph.

## Say it plainly

Start with the finding or the action. Use contractions, and pick concrete words over emphatic ones.
Say "is" instead of "serves as". Keep the qualifiers that carry meaning, such as "only on a cache
hit" or "per unit", and cut the ones that are only hedging. Use few em dashes.

Leave these out:

- previews of what a section will say, and recaps at the end
- "it's not X, it's Y" contrasts, and rhetorical questions answered on the spot
- words doing emphasis work rather than carrying meaning, such as robust, seamless, comprehensive,
  crucial, leverage, utilize and delve, and filler such as "it's worth noting"
- claims credited to "experts" or "best practice" without a named source
- emoji, decorative arrows and smart quotes
- the history of how a design got here, unless the live design looks wrong without it

A doc states what's true of the product. Progress through a plan, such as "in progress" or a
roadmap's stage names, belongs in the planning record outside this repository. A capability's status
is the exception, in the vocabulary [the capability-pages guide](reference/capability-pages.md#status)
defines.

## Code comments

Comments follow the same rules, at the scale of a line. A comment explains the constraint or failure
the code guards against, never what the code does. Don't restate a signature, narrate steps, add
divider comments or comment every function regardless of complexity.

- No sentence in a comment starts with a capital letter unless the word needs one: a name, an
  acronym, an identifier or a marker such as `TODO:`. That covers `//`, `///`, `//!`, `/* */` and
  `#` alike, and every sentence, not only the first. Pages and headings keep normal capitalization.
- A comment is true of the whole file it sits in, not just its hunk. Touching a file brings every
  comment in it into line with this section, including comments you didn't write.
- A module's `//!` comment says in plain words what it owns, never just its name.
- An `unsafe` block carries a `// SAFETY:` comment that says why each requirement of the call holds.
- A history comment says in two lines or fewer what broke, as an event, and names something
  checkable: a revision, an issue, a test or an upstream version and file. It belongs only where the
  live code reads wrong without it.
- A claim about another system's internals names the upstream version and file it was read at.
- A TODO names the concrete condition it waits on.
- No attribution, model or tool names, task ids or plan stage labels in source or commit messages.

Fixed legal and third-party notices are exempt and stay exactly as written.

## Before you finish

- The page opens with what the reader needs, then a diagram, snippet or table.
- It answers one page type, and links out for the others.
- No paragraph runs long, and no table cell is a sentence.
- Every snippet came from something that runs.
- Every link resolves, which the gate's `links` check holds.
- Nothing names a private consumer or the maintainer's machines, which the `private-names` check holds
  for exact names and your review holds for descriptions.
- Every comment in a file you touched still holds against the whole file.
