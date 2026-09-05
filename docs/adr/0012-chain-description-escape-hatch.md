---
status: "accepted"
date: 2026-09-05
decision-makers: avio maintainers
---

# The filter-string escape hatch takes a whole description, and is validated eagerly

## Context and Problem Statement

#1601 asks for an escape hatch that parses a raw libavfilter description into a graph,
"so users can reach any FFmpeg filter that does not yet have a typed builder, without
abandoning the typed API".

Measurement showed the headline need is already met. `FilterGraphBuilder::raw_filter(name,
args)` and `FilterStep::Raw { filter, args }` have existed since #1376: any untyped
one-in/one-out avfilter is reachable today, and the issue's own example
`"hue=s=0,scale=1280:720"` is `.raw_filter("hue","s=0").raw_filter("scale","1280:720")`.

So the question is not "how do we let callers reach an untyped filter" but "what, if
anything, is still out of reach" — and, once something is added, how a string that carries
no compile-time guarantees at all should be checked.

## Decision Drivers

* Two things genuinely cannot be expressed today: a chain as a **single string**, and a
  description that **branches and rejoins** (`split[a][b];[a]…[c];[b][c]overlay`). The
  `FilterStep` list is linear by construction, so no amount of per-filter escape hatches
  reaches the second.
* A description is opaque: nothing in the type system checks a filter name, an option name,
  or that an option still means what it meant two `FFmpeg` releases ago.
* The crate's established model is that `build()` constructs and validates nothing —
  `FFmpeg` first sees a filter's arguments on the first push.
* Whatever is added must not weaken the typed API's standing as the recommended path.

## Considered Options

* Parse the description with `avfilter_graph_parse2` and splice it in as one step
* Split the string on commas into `FilterStep::Raw` steps, with no new FFI
* Decline the issue as already satisfied by `raw_filter`

## Decision Outcome

Chosen option: **parse with `avfilter_graph_parse2`**, exposed as
`FilterGraphBuilder::parse_desc` (a step, so it composes with typed steps) plus a thin
`FilterGraph::parse_desc` convenience. `FilterStep::Raw` keeps the single-filter case; the
new variant `FilterStep::ParseDesc` is documented as being for a whole chain or a
non-linear description, and points at `raw_filter` for anything smaller.

The description must leave **exactly one open input and one open output**, so it links into
the chain like any other step. Sources, sinks and multi-output descriptions are rejected by
name rather than mis-linked.

Second decision, and the one that departs from the crate's norm: the description is
**validated at `build()`**, by parsing it into a scratch `AVFilterGraph` that is thrown
away. A failure returns `FilterError::InvalidConfig` naming the offending description. Every
other step still defers to the first push. The asymmetry is deliberate: a typed step's
arguments were produced by checked Rust code, a description was typed by a human, and
parsing needs no frame format so there is nothing to wait for. Where the filter registry is
not yet populated — some Linux `FFmpeg` builds report every filter as missing until an
`AVFilterGraph` exists — the check is skipped and deferred, which can only lose an early
error, never invent one.

### Confirmation

`parse_desc_should_reject_an_unknown_filter_at_build_time` in
`crates/ff-filter/tests/parse_desc_tests.rs` fails if the eager check is removed: it asserts
that `build()` — not the first push — is what returns `InvalidConfig`. Verified by removing
the `validate_parse_descs` call, which turns that test red.

`parse_desc_should_accept_a_branching_description` fails if the parse2 implementation is
replaced by comma-splitting, because a branching description is not a chain of steps.

`parse_desc_should_reject_a_description_that_is_not_one_in_one_out` fails if the arity
contract stops being enforced, and
`parse_desc_should_build_a_working_graph_from_a_chain_description` fails if the spliced
sub-graph is linked by its entry rather than its exit.

### Consequences

* Good, because the escape hatch adds exactly the two capabilities that were missing and
  duplicates nothing: `raw_filter` still owns the single-filter case.
* Good, because a typo in a description is reported at `build()`, naming the description,
  instead of surfacing as a frame-time failure with no context.
* Good, because a description composes with typed steps in one chain, which is what #1601
  asked for; it is not an alternative graph model.
* Bad, because `build()` is no longer uniformly free of `FFmpeg` work. The `build()`
  documentation states the exception.
* Bad, because a description bypasses every type-level guarantee the crate offers. The
  documentation on both entry points says so in those terms and points back at the typed
  methods.
* Surprising, because `build()` ends up checking *more* for a description than for a typed
  step. `avfilter_graph_parse2` applies options while parsing, so an unknown option name or
  a value `av_opt_set` rejects fails at `build()` — measured: `"hue=nosuchopt=1"` gives
  "Option not found", `"hue=s=notanumber"` gives "Invalid argument", while
  `raw_filter("hue", "nosuchopt=1")` still builds and fails on the first push. What remains
  push-time for a description is only what the link configuration decides (format
  negotiation, `config_props`), so a successful `build()` still does not mean the graph will
  run.
* What would reverse the arity contract: a real caller needing a multi-output description.
  That would need a different return shape than "the context the next step links to", so it
  is a larger change than relaxing a check.

## Pros and Cons of the Options

### Parse with `avfilter_graph_parse2`

* Good, because it is the same parser `ffmpeg -vf` uses, so a description that works on the
  command line works here.
* Good, because branching descriptions come for free; nothing else reaches them.
* Bad, because it adds `unsafe`, and an `AVFilterInOut` list that must be freed on every
  path including the failed-parse one.

### Split the string into `FilterStep::Raw` steps

* Good, because it needs no new FFI and no new `unsafe`.
* Bad, because it cannot express a branching description at all, which is half of what was
  missing.
* Bad, because splitting on commas means re-implementing libavfilter's escaping rules, and
  getting them subtly wrong is worse than not offering the feature.

### Decline as already satisfied

* Good, because `raw_filter` really does cover the issue's stated motivation.
* Bad, because the two gaps above are real, and the issue's acceptance criteria name
  `FilterGraph::parse_desc` specifically.

## More Information

* Issue #1601; `raw_filter` / `FilterStep::Raw` from #1376.
* `crates/ff-filter/src/filter_inner/build.rs` — `parse_desc_pads` (the parse, the arity
  contract, and why the inout lists are freed unconditionally) and `add_parse_desc_chain`.
* `crates/ff-filter/src/filter_inner/mod.rs` — `validate_parse_descs` and the registry
  probe that makes the eager check safe on a minimal `FFmpeg` build.
