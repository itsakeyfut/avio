---
status: "accepted"
date: 2026-09-04
decision-makers: avio maintainers
---

# Bitstream filters are exposed for explicit use only; libavformat keeps selecting the container-required ones

## Context and Problem Statement

Stream copy across containers sometimes needs the bitstream rewritten in place — copying
H.264 from MP4 into MPEG-TS needs the length-prefixed AVCC form turned into Annex B. #1602
proposed that `ff-remux` detect such pairings and apply the matching filter itself, on the
premise that copying packets verbatim produces broken output today.

Measurement showed the premise does not hold: libavformat already does this selection. The
question this record settles is therefore not "how do we implement automatic selection" but
"do we implement it at all, given something else already has".

## Decision Drivers

* libavformat's automatic path is unconditional on our side and cannot be opted out of per
  call, so a second implementation would run *in addition to* it, not instead of it.
* `extract_extradata`, `dump_extra` and the `*_metadata` filters are never inserted
  automatically and are unreachable today — that gap is real regardless of this decision.
* `ff-sys` owns the FFmpeg contact surface, so the `AVBSFContext` lifecycle belongs there
  whichever way the higher-level question goes.

## Considered Options

* Expose only an explicit selector; leave automatic selection to libavformat
* Re-implement automatic container/codec selection in `ff-remux`, alongside the explicit
  selector
* Wrap `AVBSFContext` in `ff-sys` and expose nothing in `ff-remux`

## Decision Outcome

Chosen option: **expose only an explicit selector**. `ff-sys` gains `BsfContext`, an owned
`AVBSFContext`; `ff-remux`'s stream-copy trim gains `video_bsf` / `audio_bsf`, taking the
same chain syntax as `ffmpeg -bsf`. No code in avio decides *which* filter a container
needs.

Three pieces of evidence, all against `FFmpeg` `release/8.0`:

1. `libavformat/mpegtsenc.c`'s `mpegts_check_bitstream` calls
   `ff_stream_add_bitstream_filter(st, e->bsf_name, NULL)` itself, with `h264_mp4toannexb`,
   `hevc_mp4toannexb` and `vvc_mp4toannexb` in its table.
2. `libavformat/mux.c` reaches that callback from `write_packets_common`, which both
   `av_write_frame` and `av_interleaved_write_frame` call. The only gate is
   `AVFMT_FLAG_AUTO_BSF`, which is set by default. `ff-remux` writes through
   `av_interleaved_write_frame` on every path.
3. Running it: the existing `StreamCopyTrimmer` copying `assets/video/gameplay.mp4`
   (H.264 + AAC) into `.ts` yields MPEG-TS sync bytes at 0/188/376 and Annex B `SPS`/`PPS`
   start codes, and demuxes back as H.264 1920x1080 + AAC — with no bitstream filter
   requested.

### Confirmation

`mp4_to_mpegts_stream_copy_should_produce_annex_b_h264` in
`crates/ff-remux/tests/bsf_tests.rs` fails if the automatic path stops working for us —
if the packet loop stops going through `av_interleaved_write_frame`, or if
`AVFMT_FLAG_AUTO_BSF` is cleared on the output context. It is gated on the MPEG-TS muxer
being present, which is a different capability from the bitstream form it asserts.

`trim_with_an_explicit_video_bsf_should_change_the_output` in the same file fails if the
explicit selector is accepted but never applied.

### Consequences

* Good, because there is one implementation of "which filter does this container need",
  and it is the one that ships with the muxer and changes with it.
* Good, because the filters that genuinely need a caller — `extract_extradata`,
  `dump_extra`, the `*_metadata` family — become reachable, which is the part that was
  actually missing.
* Bad, because a caller reading `video_bsf` may assume it is *required* for a container
  change. The method's own documentation says it is not.
* Bad, because avio inherits libavformat's choice of filter without a say in it. That has
  not been a problem, and overriding it would mean clearing `AVFMT_FLAG_AUTO_BSF`, which
  is a larger decision than this one.
* What would reverse this: a container/codec pairing where libavformat's automatic
  selection is absent or wrong and the output is measurably broken. Point 3 above is the
  measurement to repeat.

## Pros and Cons of the Options

### Expose only an explicit selector

* Good, because it adds exactly the capability that is missing and nothing else.
* Good, because it cannot conflict with libavformat: an explicitly requested filter runs
  before the muxer sees the packet, and the muxer's own check then finds nothing left to do.
* Bad, because it does not make the issue's headline case (MP4 → MPEG-TS) any better —
  that case was already correct.

### Re-implement automatic selection in `ff-remux`

* Good, because the selection would be visible in avio's own code rather than inherited.
* Bad, because it duplicates a table that lives with the muxers and grows with them
  (`vvc_mp4toannexb` was added to that table for VVC; avio would have to track it).
* Bad, because it changes nothing observable: measured against the three points above, the
  output is already correct.

### Wrap `AVBSFContext` in `ff-sys` and expose nothing in `ff-remux`

* Good, because the primitive is the durable part, and the first named consumer in #1602
  is `ff-stream`'s TS segmentation, not `ff-remux`.
* Bad, because `extract_extradata` / `dump_extra` would stay unreachable, which is the one
  acceptance criterion of #1602 that describes a real gap.

## More Information

* Issue #1602, and the `ff-stream` TS-segmentation follow-up it names.
* `crates/ff-sys/src/bsf.rs` — the `BsfContext` lifecycle and why its send/receive are safe
  functions.
* `crates/ff-remux/src/trim/trim_inner.rs` — where a requested filter sits in the packet
  path, and why the output stream is configured from the filter's `par_out`.
