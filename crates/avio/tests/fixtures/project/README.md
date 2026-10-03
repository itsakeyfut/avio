# Project format fixtures

One document per released format version, loaded by `tests/project_format.rs`. These are the only test
that catches a format break, which is why they accumulate rather than being regenerated.

| File | Version | What it is |
|---|---|---|
| `v0.json` | 0 | No envelope: the model serialised on its own, which is what a caller reaching for `serde_json` produced before `Project` existed. Every document written by 0.18.4 and earlier looks like this. |
| `v1.json` | 1 | The envelope, introduced in 0.19.0. Its `frame_rate` is the decimal `30.0`. |
| `v1-ntsc.json` | 1 | The same version 1 document with `"frame_rate": 29.97`. It sits beside `v1.json` rather than replacing it because it is the only fixture that exercises the interesting branch of the version 1 step: an integer rate converts the same way under either reading of a decimal, so `v1.json` alone cannot tell a correct migration from one that takes `29.97` literally as `2997/100`. |
| `v2.json` | 2 | `frame_rate` as a ratio (`{"num": 30, "den": 1}`), from 0.19.0. |

## Adding one

When `PROJECT_FORMAT_VERSION` increments, add `v<n>.json` **and leave the older files alone**. An older
fixture is the record of a shape that shipped; editing it to match a later shape removes the only
evidence that the migration from it works.

Generate the new one by serialising the model at that version rather than by editing the previous file
by hand, so the fixture is what the code actually writes:

```rust
std::fs::write("v<n>.json", Project::new(timeline).to_json_string()?)?;
```

`v0.json`, `v1.json` and `v2.json` were produced that way, from a 640x360 30 fps timeline holding one
file-backed clip trimmed to two seconds. `v1-ntsc.json` is `v1.json` with one field changed, because no
code can serialise a version 1 document any more; a fixture for a version that has shipped is derived
from the one beside it rather than regenerated.
