# avio Roadmap

What each milestone delivers, and where the direction after it is recorded. [`README.md`](../README.md)
says what avio is, which crates it publishes and how to install them; that is not repeated here.

A milestone's `ROADMAP.md` states the **capabilities** the version delivers, not the tasks that deliver
them. The tasks are GitHub issues in the matching milestone, which is the only place their status is
accurate.

Current workspace version: **0.18.4**. In progress: **v0.19.0**.

## Milestones

| Version | Theme | Status |
|---|---|---|
| [v0.1.x](roadmap/v0-1-x/ROADMAP.md) | Stabilization and quality | Released |
| [v0.2.0](roadmap/v0-2-0/ROADMAP.md) | `ff-filter` core | Released |
| [v0.3.0](roadmap/v0-3-0/ROADMAP.md) | Encoding enhancements, metadata write | Released |
| [v0.4.0](roadmap/v0-4-0/ROADMAP.md) | `ff-pipeline` | Released |
| [v0.5.0](roadmap/v0-5-0/ROADMAP.md) | `ff-stream` | Released |
| [v0.6.0](roadmap/v0-6-0/ROADMAP.md) | Async support (opt-in `tokio` feature) | Released |
| [v0.7.0](roadmap/v0-7-0/ROADMAP.md) | Advanced codec options, professional formats | Released |
| [v0.8.0](roadmap/v0-8-0/ROADMAP.md) | Network input, live streaming | Released |
| [v0.9.0](roadmap/v0-9-0/ROADMAP.md) | Advanced filtering, effects, clip processing | Released |
| [v0.10.0](roadmap/v0-10-0/ROADMAP.md) | Multi-track composition, media analysis | Released |
| [v0.11.0](roadmap/v0-11-0/ROADMAP.md) | Compositing, keying, blend modes | Released |
| [v0.12.0](roadmap/v0-12-0/ROADMAP.md) | Keyframe animation | Released |
| [v0.13.0](roadmap/v0-13-0/ROADMAP.md) | Real-time preview, proxy workflow | Released |
| [v0.14.0](roadmap/v0-14-0/ROADMAP.md) | Advanced effects, audio processing | Released |
| [v0.15.0](roadmap/v0-15-0/ROADMAP.md) | FFmpeg token canonicalization | Released |
| [v0.16.0](roadmap/v0-16-0/ROADMAP.md) | Engine / library split, independent publishing | Released |
| [v0.17.0](roadmap/v0-17-0/ROADMAP.md) | Editing model maturity, library hardening | Released |
| [v0.18.0](roadmap/v0-18-0/ROADMAP.md) | Editing model and GPU rendering | Released (see below) |
| [v0.19.0](roadmap/v0-19-0/ROADMAP.md) | Editing depth, delivery parity, the cost of an edit | In progress |
| v0.20.0 | Alpha and canvas handling | Planned |
| [v1.0.0](roadmap/v1-0-0/ROADMAP.md) | Stable API | Planned |

**v0.18.0 shipped and its milestone is not empty.** The 0.18.x patch releases fixed part of it, and the
rest are bugs found after the release plus the milestone's tracking issues. The milestone is the record
of what that version's scope turned out to contain, so the open issues are left there rather than
back-dated into a version that has already shipped; where they get fixed is decided per issue.

Two earlier milestones carry no directory here because their scope is recorded only as issues:
`type-consolidation`, which was a single refactor, and v0.20.0, which is still being shaped.

## Beyond the next release

[`roadmap/plan.md`](roadmap/plan.md) records the long-term direction. It is a living reference and not a
commitment: v0.19.0 and v0.20.0 in it were both re-scoped after it was written, and the versions past
them have not been tested against what the engine actually needs yet.

Work that is wanted but not scheduled lives in backlog milestones on GitHub rather than in this file, so
that it has issue numbers and can be pulled into a release without being retyped:
`backlog-architecture`, `backlog-audio-music` and `backlog-interchange-gpu`. The last one has notes in
[`roadmap/backlog/interchange-and-gpu.md`](roadmap/backlog/interchange-and-gpu.md).

## Contributing

Open an issue before starting work on anything above, so that scope is agreed before code is written. An
issue labelled `S-Needs-Design` has its design settled first, as a comment on the issue.
