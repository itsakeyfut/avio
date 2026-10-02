# Project format fixtures

One document per released format version, loaded by `tests/project_format.rs`. These are the only test
that catches a format break, which is why they accumulate rather than being regenerated.

| File | Version | What it is |
|---|---|---|
| `v0.json` | 0 | No envelope: the model serialised on its own, which is what a caller reaching for `serde_json` produced before `Project` existed. Every document written by 0.18.4 and earlier looks like this. |
| `v1.json` | 1 | The envelope, introduced in 0.19.0. |

## Adding one

When `PROJECT_FORMAT_VERSION` increments, add `v<n>.json` **and leave the older files alone**. An older
fixture is the record of a shape that shipped; editing it to match a later shape removes the only
evidence that the migration from it works.

Generate the new one by serialising the model at that version rather than by editing the previous file
by hand, so the fixture is what the code actually writes:

```rust
std::fs::write("v<n>.json", Project::new(timeline).to_json_string()?)?;
```

Both files here were produced that way, from a 640x360 30 fps timeline holding one file-backed clip
trimmed to two seconds.
