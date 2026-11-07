# Nix style

How Nix in this repository is designed: values before syntax, kernels and stages, laziness, the
module system, layout and list order. Read it with [the writing guide](../writing.md#code-comments),
which owns comments.

The gate's `format` check formats every `.nix` file with nixfmt, and its `nix-lint` check lints them
with statix and deadnix, as [the root AGENTS.md](../../AGENTS.md#gates) lists. A formatter or lint
finding is settled by the tool, not by restyling around it. This page covers what the tools can't
check: how Nix code is designed, not how it's laid out.

When style and substance disagree, the order is fixed. Correctness and the owner's invariants come
first, measured evaluation cost second and appearance last. This reference ranks below AGENTS.md
and the writing guide wherever they speak.

## Design from values, not from syntax

Nix code here is functional code that happens to be written in Nix. Start with the domain value,
its invariants and the operations that consume it. Then choose the construct that expresses that
shape. Don't start from an attribute set and keep adding fields and branches until it works.

| Shape | Construct |
| --- | --- |
| Values needed together | A record with a declared type |
| One of several alternatives | A tagged sum: `{ _tag = "grouped"; ... }` or a typed variant |
| A constrained value | A refinement: a base type plus a named predicate |
| A step that can fail on user input | A stage that validates the input once and returns diagnostics naming where it came from |
| A pure algorithm over checked data | A plain function |

## Kernels and stages

Keep computation and failure reporting in separate functions.

- **A kernel** is a pure function over data that's already valid, such as a merge, a selection, an
  index or a renderer. It uses no effects and reports no diagnostics. It throws only when its caller
  broke its contract.
- **A stage** validates user input, calls kernels and reports diagnostics. It's thin.

A function that both computes something hard and reports user errors is two functions. Kernels are
where evaluation cost lives, so they're what an algorithm audit reads.

## Vocabulary over repetition

Nix has no pattern matching and no type system in the language, so the same branching shapes get
written again and again. Name a repeated shape once and reuse it.

- **`match`** over a tagged sum replaces an `if`/`else if` chain on a tag. It checks that every
  constructor is handled when the matcher is built, so a new variant fails loudly instead of falling
  into `else`.
- **A fold** over a tree replaces a hand-written recursive walk, with one traversal order and children
  visited only on demand.
- **Dispatch by registered name** replaces `if name == ...` chains. It rejects unknown names before
  lookup and checks the table for collisions.

Add a combinator when a shape repeats across owners and the combinator guarantees something the
hand-written version can't. Keep `if` for guards, short-circuits and one-off decisions. A dispatch
table in place of a clear two-way `if` is worse code.

## Laziness

Laziness is part of a function's contract. State what an operation forces when it isn't obvious.

- Keep inactive payloads, native module bodies and unselected outputs unforced.
- Don't `deepSeq`, `toJSON` or `tryEval` a value only to improve a message or pass it between pure
  steps.
- Let a callback's exception surface at the expression that forced it. Don't convert evaluation
  failures into a generic error.
- Test laziness by passing throwing values through the whole path.

## The module system

Use the nixpkgs module system where the consumer is a module system: NixOS, Home Manager,
flake-parts. Don't use it as a library's internal configuration engine. A library's own settings
are plain schemas validated once, and a module-system user reaches them through a thin adapter at
the edge.

## Evaluation is pure

Don't read mutable files during evaluation or use import-from-derivation to move work into
evaluation. Filesystem changes, process execution and locking belong to runtime services that
consume a plan the evaluation produced.

## Layout and naming

- A file owns one concern, and its directory names that concern. `utils`, `helpers`, `common` and
  `lib` as a catch-all name no responsibility.
- Use `with` for expressions that repeatedly use one namespace. Package lists use
  `with pkgs; [ ... ]` instead of repeating `pkgs.` on every element. Apply the same rule to
  other namespaces when it makes the expression clearer, and keep the scope local to that
  expression. Use explicit qualification where mixed namespaces would make a name ambiguous.
- Use `inherit` to copy named attributes into a set.
- Don't use `|>` until CppNix, Determinate Nix and Lix all accept it without an experimental
  feature. Shikumi runs on all three, and an expression that needs a feature turned on fails to parse
  wherever it's off.
- Build strings and lists from fragments joined once. Don't grow them inside a fold.

## List order

When the order of a list doesn't affect behavior, order its elements by rendered length, longest
first. That covers package lists, `runtimeInputs`, input names and similar sets written as lists.

```nix
packages = with pkgs; [
  rust-analyzer
  deadnix
  nixfmt
  statix
  jq
];
```

Keep the authored order whenever order means something, and treat a list as ordered when you
aren't sure. Ordered lists include task `steps`, overlays, `PATH`-like search lists, layer lists
folded last-wins, and module `imports` whose definitions merge into a list-typed option.

No tool in Shikumi's gate sorts or checks list order. nixfmt lays a list out without reordering it,
and statix and deadnix don't look at order, so this rule is held in review. The package lists in
`flake.nix` follow it.
