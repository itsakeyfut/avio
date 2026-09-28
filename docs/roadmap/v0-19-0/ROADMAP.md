# v0.19.0: Editing Depth, Delivery Parity, and the Cost of an Edit

The version where avio stops being an engine that can render a timeline and becomes one an editor can
be built on. Three things are true at the start of it, and each is a theme:

- **The editing model can express less than an editor needs.** There is no insert or overwrite, no
  roll/slip/slide, no way to duplicate a clip or disable one, and a bulk change is many undo steps.
- **The engine reaches only part of what the primitives already do.** `EncoderConfig` exposes seven
  knobs while `ff-encode` carries colour tagging, faststart, HDR signalling and profiles; the render
  hardcodes 48 kHz stereo and cannot choose a pixel format, so ProRes, DNxHD, 10-bit and alpha
  deliverables are unreachable through the engine.
- **Nobody has measured what an edit costs.** The export runs decode, filter and encode in one thread
  one frame at a time, a tiling track pays for every clip on every frame, and each clip opens its own
  decoder. None of that has a number attached to it yet.

This version is deliberately large. A minor bump here is a substantial addition to what the engine can
do, not a handful of fixes.

Scope is 47 issues in the `v0.19.0` milestone. Every one of them starts `S-Needs-Design`: the design is
settled as a comment on the issue before any of it is implemented.

## What you can do after this version

### Edit the way an editor expects

Three-point editing with insert and overwrite (#1825). The three trims the model cannot currently
express, roll, slip and slide (#1876). Duplicate and paste a clip so a pattern is reused rather than
rebuilt (#1877). Take a shot out without deleting it (#1878). Drop two clips so they overlap and get a
transition from it (#1897). Apply several commands as **one** undo step, so a bulk change undoes the way
the user made it (#1885).

### Address time exactly

Frame-exact addressing and timecode, so a position means one frame and not a float that rounds (#1827).
Speed keyframed like every other clip property, rather than one static value per clip (#1874). Export a
range of the timeline instead of always the whole programme (#1875).

### Place a sound on a beat

The engine can express every edit a music-driven video needs and still cannot be used to make one,
because the positions have to be computed outside it. Musical time, so a clip goes on beat 3.5 of bar 12
rather than at a hand-computed 4.137931 seconds (#1914). The tempo is already detectable and was never
usable: `BpmResult` is re-exported and nothing consumes it. And placement that keeps the precision the
trim already keeps: the export truncates a clip's audio offset to whole milliseconds, so every clip
starts up to a millisecond early, one-directionally (#1915).

This pair is what turns the milestone's other work into something an editor of chopped-sample material
can use. It is also why #1871 and #1872 matter here rather than being ordinary optimisation: hundreds of
short clips cut from one file is that material's normal shape.

### Deliver what the primitives can already produce

A pixel format the deliverable requires, which is what ProRes, DNxHD, 10-bit and alpha need (#1808).
Audio at the layout and rate the delivery calls for, not 48 kHz stereo only (#1823). The encoder
settings `ff-encode` has carried all along (#1888). More than one audio stream, and subtitles that are
not burned in (#1892). Several deliverables from one edit, queued (#1900).

### Keep a project working when its files move

A saved project that carries a format version, so this version's model changes do not break every
existing project (#1905). This one gates the rest of the model work. An asset model, so a project
survives its media moving (#1828). Replace a clip's source while keeping its trim, effects and
keyframes (#1894). See where a source is used before anything is replaced or relinked (#1902). Hold
several candidate takes in one slot and switch between them (#1895). Apply rules when media is brought
in, instead of repeating the same setup per file (#1904).

### Put a title on screen

The settings a title actually needs: outline, shadow, alignment, weight (#1824). A shape to sit a
caption on (#1906). Rotate and scale about a chosen point rather than always the centre (#1907).

### Turn what the analysers find into edits

The detectors exist and produce data nothing acts on. Cut silence automatically (#1880). Split a clip at
detected scene changes (#1881). Derive a colour correction from the picture's own histogram (#1882). Duck
music under the speech the detector already finds (#1883). Trim the dead head and tail off a clip
(#1884). Match one shot's colour to another's (#1901).

### Author curves by editing, not by hand

Record a keyframe by changing a property at a position, rather than authoring the track by hand (#1896).
Read and set a keyframe's tangents, so a host can draw and edit the curve (#1903).

### Structure a timeline

Track properties as commands: mute, solo, lock, rename, reorder (#1826). Nested timelines, so a sequence
can be used as a clip (#1829). Adjustment layers, so one effect grades everything beneath it (#1879).
Effect presets, so a configured look is reused instead of rebuilt (#1886).

### Know, and then reduce, what an edit costs

**Profiling comes first (#1873).** The optimisation target is chosen from a measurement, not from
reading the code, and the issues below are designed against that measurement rather than ahead of it.
What is then addressed: a tiling track paying for every clip on every frame (#1871), one decoder per
clip where clips share a file (#1872), software decode while the encoder is on the GPU (#1889),
re-encoding a section nothing touched (#1890), decode/filter/encode serialised in one thread (#1891), and
a heavy section that cannot be scrubbed because nothing is cached (#1899).

### Survive bad input

A corrupt frame skipped rather than ending the render (#1893). An override for how a source is
interpreted, when the file's own tags are wrong (#1898).

### Stop adding a variant being a breaking change

Which public enums are `#[non_exhaustive]`, decided rather than inherited (#1887).

## How this version is verified

The same way its predecessors were, with two rules this milestone's issues were filed under:

- **A performance claim carries a measurement.** An issue asserting a cost states the number it was
  measured at and against which baseline. Where the number does not exist yet, the issue is an
  experiment (`T-Experiment`) before it is an implementation.
- **A duration is not evidence.** An export's length is fixed by the canvas, so a test that asserts it
  passes whatever the picture does. Placement and length are asserted in frames and samples.

## Out of scope

Alpha and canvas handling moved to v0.20.0 as its own milestone. Interchange formats (AAF, EDL, OTIO)
and further GPU acceleration remain in `backlog-interchange-gpu`. An AI/ML crate with user-supplied
models is a direction the author has named, not a decision, and is not in this version.
