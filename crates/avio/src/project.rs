//! The project file: a [`Timeline`] wrapped in an envelope that carries the format
//! version, and the migration chain that reads an older one.
//!
//! # Why an envelope
//!
//! `Timeline` derives `Serialize`/`Deserialize`, so a caller has always been able to
//! persist the model by reaching for `serde_json` directly. Nothing in that output says
//! which version of the model wrote it, which is fine for exactly as long as the types
//! happen to stay compatible (#1905). The envelope fixes that by carrying the version
//! outside the model:
//!
//! ```json
//! { "format_version": 1, "timeline": { ... } }
//! ```
//!
//! The outer shape is frozen from this release on. The model inside it is not, which is
//! the point: a release may change the model and add a migration step, and the loader
//! reads the version before it reads the model so it knows which steps to run.
//!
//! **A bare serialised `Timeline` is not a project file.** It has no envelope, so it has
//! no version, so nothing can know how to read it in three releases' time.
//! [`Project::from_json_str`] accepts one anyway, as [version 0](Project::load), because
//! that is what documents written before this release look like.
//!
//! # Why the format is the derive output
//!
//! The alternative is an explicit schema the model converts to and from, which buys the
//! freedom to restructure the model without touching the format. It costs hand-written
//! conversion for the 80 types reachable from `Timeline` (53 in `avio`, 9 in `ff-format`,
//! 18 in `ff-filter`), one of which carries 99 variants, maintained in step with the
//! model forever. ADR-0024 records the choice and what it rules out.
//!
//! What the derive costs is that the Rust shape **is** the format, so a rename or a
//! restructure is a format change. That is what the migration chain is for.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use ff_format::Rational;

use crate::timeline::Timeline;

/// The format version this release writes, and the highest it can read.
pub const PROJECT_FORMAT_VERSION: u32 = 2;

/// The version a document carries when it has no `format_version` at all, which is every
/// document written before the envelope existed.
const UNVERSIONED: u32 = 0;

/// The envelope's field names, which are frozen and must not be renamed.
const FIELD_VERSION: &str = "format_version";
const FIELD_TIMELINE: &str = "timeline";
/// The model field version 2 changed from a number to a ratio.
const FIELD_FRAME_RATE: &str = "frame_rate";

/// A saved project: the editing document plus the format version it was written with.
///
/// ```no_run
/// use avio::{Clip, Project, Timeline};
///
/// let timeline = Timeline::builder()
///     .canvas(1920, 1080)
///     .frame_rate(30.into())
///     .video_track(vec![Clip::new("clip.mp4")])
///     .build()?;
/// Project::new(timeline).save("song.avio")?;
///
/// let project = Project::load("song.avio")?;
/// let timeline = project.into_timeline();
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Project {
    timeline: Timeline,
}

impl Project {
    /// Wraps `timeline` so it can be saved.
    #[must_use]
    pub const fn new(timeline: Timeline) -> Self {
        Self { timeline }
    }

    /// The document this project holds.
    #[must_use]
    pub const fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// Takes the document, consuming the project.
    #[must_use]
    pub fn into_timeline(self) -> Timeline {
        self.timeline
    }

    /// Serialises the project, envelope and all.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Malformed`] when the model cannot be serialised.
    pub fn to_json_string(&self) -> Result<String, ProjectError> {
        let envelope = Envelope {
            format_version: PROJECT_FORMAT_VERSION,
            timeline: &self.timeline,
        };
        serde_json::to_string_pretty(&envelope).map_err(|e| ProjectError::Malformed {
            reason: e.to_string(),
        })
    }

    /// Reads a project, migrating it forward from whatever version wrote it.
    ///
    /// A document with no `format_version` is read as version 0, which is what every
    /// document written before this release is.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Malformed`] when the text is not a document this can read,
    /// [`ProjectError::VersionTooNew`] when a later release wrote it, and
    /// [`ProjectError::MigrationFailed`] when a step could not convert it.
    pub fn from_json_str(text: &str) -> Result<Self, ProjectError> {
        let document: Value = serde_json::from_str(text).map_err(|e| ProjectError::Malformed {
            reason: e.to_string(),
        })?;
        let version = document_version(&document)?;
        let migrated = migrate(document, version, PROJECT_FORMAT_VERSION, STEPS)?;
        let timeline = timeline_from(&migrated)?;
        Ok(Self { timeline })
    }

    /// Writes the project to `path`.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Io`] when the file cannot be written, and
    /// [`ProjectError::Malformed`] when the model cannot be serialised.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let text = self.to_json_string()?;
        std::fs::write(path, text).map_err(ProjectError::Io)
    }

    /// Reads the project at `path`, migrating it forward.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Io`] when the file cannot be read, plus everything
    /// [`from_json_str`](Self::from_json_str) returns.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ProjectError> {
        let text = std::fs::read_to_string(path).map_err(ProjectError::Io)?;
        Self::from_json_str(&text)
    }
}

/// The on-disk shape. Borrowed on the way out so saving does not clone the model.
#[derive(Serialize)]
struct Envelope<'a> {
    format_version: u32,
    timeline: &'a Timeline,
}

/// What went wrong saving or reading a project.
///
/// Separate from [`TimelineError`](crate::TimelineError) for the reason
/// [`EditError`](crate::EditError) is: a failure to read a file is a different domain
/// from a failure to render, and a caller that handles one has no use for the other's
/// variants.
/// `#[non_exhaustive]` because this enum is new here. Adding a variant to one of the
/// crate's older error types breaks a downstream exhaustive match (#1887); doing it now,
/// before anyone matches on this one, costs nothing and keeps the next variant additive.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProjectError {
    /// The file could not be read or written.
    #[error("project file i/o failed: {0}")]
    Io(#[source] std::io::Error),

    /// The text is not a project document, or the model inside it could not be read.
    ///
    /// Carries the reason as a string rather than the underlying `serde_json` error, so
    /// that crate stays out of this one's public API.
    #[error("project document is malformed: {reason}")]
    Malformed {
        /// What the parser or the model's own deserialiser reported.
        reason: String,
    },

    /// A later release wrote this document.
    ///
    /// Refused rather than read as far as it goes: a later release may mean something
    /// different by the same field, and a reader that guesses produces a wrong project
    /// instead of no project.
    #[error(
        "project format version {document} is newer than this release can read \
         (supported up to {reader})"
    )]
    VersionTooNew {
        /// The version the document carries.
        document: u32,
        /// The highest version this release reads.
        reader: u32,
    },

    /// A migration step could not convert the document.
    #[error("migrating the project from format version {from} failed: {reason}")]
    MigrationFailed {
        /// The version the step was converting from.
        from: u32,
        /// Why it could not.
        reason: String,
    },
}

/// One step of the migration chain: it converts a document at version `from` into the
/// shape version `from + 1` expects.
///
/// A step works on a [`Value`] rather than on typed snapshots of the old model, which is
/// what keeps a rename from needing a frozen copy of 80 types. A step that has to
/// understand the model can still deserialise the part it needs.
struct Step {
    /// The version this step reads.
    from: u32,
    /// The conversion.
    apply: fn(Value) -> Result<Value, ProjectError>,
}

/// The chain, in ascending order of `from`.
///
/// Adding a step never requires editing an existing one: a step describes a conversion
/// out of a version that has shipped, and a shipped version does not change.
const STEPS: &[Step] = &[
    Step {
        from: UNVERSIONED,
        apply: wrap_bare_model,
    },
    Step {
        from: 1,
        apply: rate_to_rational,
    },
];

/// The decimals that name a standard rate, and the ratio each one means.
///
/// A project that stored `29.97` was authored against NTSC, because that is what the
/// number means in this domain. Reading it literally as `2997/100` would pin a timeline
/// that was cutting NTSC material to a grid 0.1% off, and frame-exact addressing is
/// exactly what would make that error visible afterwards. Recorded as an ADR, because the
/// literal reading is defensible.
const STANDARD_RATES: &[(f64, i32, i32)] = &[
    (29.97, 30_000, 1001),
    (23.976, 24_000, 1001),
    (59.94, 60_000, 1001),
    (47.952, 48_000, 1001),
    (119.88, 120_000, 1001),
];

/// How close a stored decimal has to be to a standard rate to be read as one.
///
/// Wide enough to catch the spellings a host might have written (`29.97`, `29.970`, and
/// the `30000.0/1001.0` an `f64` division produces, which is 29.97002997...), and far
/// narrower than the gap to any neighbouring rate.
const RATE_TOLERANCE: f64 = 1e-3;

/// The largest denominator a rate that is not standard is reduced to.
///
/// A stored decimal carries at most a few places in practice, so a thousandth resolves
/// every rate anyone writes while keeping the numerator inside `i32`.
const RATE_DENOMINATOR: i32 = 1000;

/// Version 1 to 2: the model's `frame_rate` becomes a ratio.
///
/// Version 1 stored a decimal, which cannot represent the broadcast rates: no `f64`
/// spelled `29.97` is `30000/1001`, so a timeline could not say which frame a position
/// fell on (#1947). This is the chain's first step that converts rather than rewraps.
fn rate_to_rational(document: Value) -> Result<Value, ProjectError> {
    let mut document = document;
    let Some(object) = document.as_object_mut() else {
        return Err(ProjectError::MigrationFailed {
            from: 1,
            reason: "the version 1 document is not an object".to_string(),
        });
    };

    let timeline = object
        .get_mut(FIELD_TIMELINE)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ProjectError::MigrationFailed {
            from: 1,
            reason: format!("the envelope carries no `{FIELD_TIMELINE}` object"),
        })?;

    // An absent rate is left absent rather than defaulted: the model's own `Deserialize`
    // decides what a missing field means, and guessing here would put a rate into a
    // document that never had one.
    if let Some(stored) = timeline.get(FIELD_FRAME_RATE) {
        // Already a ratio: leave it. This is reachable, and not only through a corrupt
        // document. **Version 0 is defined by the absence of an envelope, not by a
        // shape**, so a bare model serialised by *this* release is read as version 0,
        // wrapped by the step from 0, and then handed to this step with its rate already
        // converted. The step from 0 cannot stamp the version whose shape the model has,
        // because it cannot know it, so every later step has to be idempotent on the
        // field it owns.
        if stored.is_object() {
            timeline.insert(FIELD_FRAME_RATE.to_string(), stored.clone());
        } else {
            let decimal = stored
                .as_f64()
                .ok_or_else(|| ProjectError::MigrationFailed {
                    from: 1,
                    reason: format!("`{FIELD_FRAME_RATE}` is not a number: {stored}"),
                })?;
            let rate =
                rational_from_decimal(decimal).ok_or_else(|| ProjectError::MigrationFailed {
                    from: 1,
                    reason: format!("`{FIELD_FRAME_RATE}` is not a usable rate: {decimal}"),
                })?;
            timeline.insert(FIELD_FRAME_RATE.to_string(), rate);
        }
    }

    object.insert(FIELD_VERSION.to_string(), Value::from(2u32));
    Ok(document)
}

/// The ratio a stored decimal rate means, or `None` when it names no rate at all.
///
/// `None` for a value that cannot be a frame rate (not finite, not positive, or too large
/// to reduce), which the caller turns into a failed migration rather than silently
/// storing a degenerate ratio.
fn rational_from_decimal(decimal: f64) -> Option<Value> {
    if !decimal.is_finite() || decimal <= 0.0 {
        return None;
    }

    for &(standard, num, den) in STANDARD_RATES {
        if (decimal - standard).abs() < RATE_TOLERANCE {
            return Some(rational_value(num, den));
        }
    }

    let scaled = (decimal * f64::from(RATE_DENOMINATOR)).round();
    if scaled < 1.0 || scaled > f64::from(i32::MAX) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the bounds check above rejects every value a numerator cannot hold"
    )]
    let num = scaled as i32;
    let reduced = Rational::new(num, RATE_DENOMINATOR).reduce();
    Some(rational_value(reduced.num(), reduced.den()))
}

/// A ratio in the shape `Rational`'s derived `Serialize` writes.
///
/// Built by hand rather than by serialising a `Rational`, so that a change to the
/// derived shape fails the fixture tests here instead of silently changing what a
/// migrated document looks like.
fn rational_value(num: i32, den: i32) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("num".to_string(), Value::from(num));
    map.insert("den".to_string(), Value::from(den));
    Value::Object(map)
}

/// Version 0 to 1: put the envelope around a document that never had one.
///
/// A version 0 document is the model serialised on its own, because that is what a caller
/// reaching for `serde_json` produced before the envelope existed. The model's own shape
/// did not change between 0.18.4 and this release, so wrapping it is the whole
/// conversion; doing it here rather than branching in the loader is what keeps every
/// migrated document the same shape.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature is Step::apply's, which `rate_to_rational` needs in order to fail"
)]
fn wrap_bare_model(model: Value) -> Result<Value, ProjectError> {
    let mut envelope = serde_json::Map::new();
    envelope.insert(FIELD_VERSION.to_string(), Value::from(1u32));
    envelope.insert(FIELD_TIMELINE.to_string(), model);
    Ok(Value::Object(envelope))
}

/// The version a document declares, or [`UNVERSIONED`] when it declares none.
///
/// An absent field means version 0, which is a document written before the envelope
/// existed. A field that is present but is not a version number is refused rather than
/// also read as 0: reading the version is the first thing this module does, and a reader
/// that guesses here reports a fault in the model to someone whose version field is what
/// is wrong.
fn document_version(document: &Value) -> Result<u32, ProjectError> {
    match document.get(FIELD_VERSION) {
        None => Ok(UNVERSIONED),
        Some(value) => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| ProjectError::Malformed {
                reason: format!("`{FIELD_VERSION}` is not a version number: {value}"),
            }),
    }
}

/// Runs every step from `from` up to `to`, in ascending order.
///
/// Both `to` and `steps` are parameters rather than being read from the constants inside.
/// That is what lets the chain's behaviour be exercised with steps that convert something
/// and with a target more than one version away: the real chain is one step long today,
/// so a test given only the constants could not tell ordering from luck.
///
/// # Errors
///
/// [`ProjectError::VersionTooNew`] when `from` is above `to`, and
/// [`ProjectError::MigrationFailed`] when a version in the range has no step, or a step
/// reports a failure.
fn migrate(document: Value, from: u32, to: u32, steps: &[Step]) -> Result<Value, ProjectError> {
    if from > to {
        return Err(ProjectError::VersionTooNew {
            document: from,
            reader: to,
        });
    }

    let mut current = document;
    for version in from..to {
        // A gap is refused rather than skipped. Skipping would hand the next step a shape
        // it was not written for, which produces a wrong project rather than a failure.
        let Some(step) = steps.iter().find(|s| s.from == version) else {
            return Err(ProjectError::MigrationFailed {
                from: version,
                reason: format!("no migration step reads format version {version}"),
            });
        };
        current = (step.apply)(current)?;
    }
    Ok(current)
}

/// Deserialises the model out of a migrated document.
///
/// Every migrated document is an envelope, including one that arrived without one, which
/// is what the version 0 step is for.
fn timeline_from(document: &Value) -> Result<Timeline, ProjectError> {
    let model = document
        .get(FIELD_TIMELINE)
        .ok_or_else(|| ProjectError::Malformed {
            reason: format!("the envelope carries no `{FIELD_TIMELINE}`"),
        })?;
    serde_json::from_value(model.clone()).map_err(|e| ProjectError::Malformed {
        reason: e.to_string(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A step that renames a field, which is what most future steps will be and what a
    /// `Value` transform exists to make cheap.
    fn rename_fps(mut document: Value) -> Result<Value, ProjectError> {
        let map = document.as_object_mut().unwrap();
        let fps = map
            .remove("fps")
            .ok_or_else(|| ProjectError::MigrationFailed {
                from: 0,
                reason: "no fps".to_string(),
            })?;
        map.insert("frame_rate".to_string(), fps);
        Ok(document)
    }

    fn note_a(mut document: Value) -> Result<Value, ProjectError> {
        push(&mut document, "a");
        Ok(document)
    }

    fn note_b(mut document: Value) -> Result<Value, ProjectError> {
        push(&mut document, "b");
        Ok(document)
    }

    fn push(document: &mut Value, name: &str) {
        document
            .as_object_mut()
            .unwrap()
            .entry("log")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!(name));
    }

    /// The non-vacuous proof that the chain converts. Nothing else here would notice a
    /// `migrate` that returned its input untouched.
    #[test]
    fn migrate_should_apply_a_step_that_changes_the_document() {
        let steps = [Step {
            from: 0,
            apply: rename_fps,
        }];

        let out = migrate(json!({ "fps": 30.0 }), 0, 1, &steps).unwrap();

        assert!(out.get("fps").is_none(), "the old name must be gone");
        assert_eq!(
            out.get("frame_rate").and_then(Value::as_f64),
            Some(30.0),
            "the value must survive under the new name"
        );
    }

    /// Two steps whose composition is order dependent, walked across two versions. This
    /// is why `migrate` takes its target as a parameter: the real chain is one step long,
    /// so ordering could not otherwise be told from luck.
    #[test]
    fn migrate_should_apply_steps_in_ascending_order() {
        let steps = [
            Step {
                from: 1,
                apply: note_b,
            },
            Step {
                from: 0,
                apply: note_a,
            },
        ];

        let out = migrate(json!({}), 0, 2, &steps).unwrap();

        assert_eq!(
            out.get("log").unwrap(),
            &json!(["a", "b"]),
            "the step from 0 runs before the step from 1, whatever order they are listed in"
        );
    }

    #[test]
    fn migrate_should_refuse_a_gap_in_the_chain() {
        // A chain that can go 0 to 1 but not 1 to 2, asked to reach 2.
        let steps = [Step {
            from: 0,
            apply: note_a,
        }];

        let err = migrate(json!({}), 0, 2, &steps).unwrap_err();

        assert!(
            matches!(err, ProjectError::MigrationFailed { from: 1, .. }),
            "the missing version belongs in the error, got {err:?}"
        );
    }

    #[test]
    fn migrate_should_refuse_a_version_newer_than_the_reader() {
        let err = migrate(
            json!({}),
            PROJECT_FORMAT_VERSION + 1,
            PROJECT_FORMAT_VERSION,
            STEPS,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                ProjectError::VersionTooNew { document, reader }
                    if document == PROJECT_FORMAT_VERSION + 1 && reader == PROJECT_FORMAT_VERSION
            ),
            "both versions belong in the error, got {err:?}"
        );
    }

    #[test]
    fn migrate_should_be_the_identity_at_the_current_version() {
        let document = json!({ FIELD_VERSION: PROJECT_FORMAT_VERSION, "untouched": true });
        let out = migrate(
            document.clone(),
            PROJECT_FORMAT_VERSION,
            PROJECT_FORMAT_VERSION,
            STEPS,
        )
        .unwrap();
        assert_eq!(out, document, "no step runs when there is nothing to cross");
    }

    /// The real chain's first step, in isolation: a version 0 document is a bare model,
    /// and the step puts the envelope around it without touching the model.
    ///
    /// Migrated to 1 rather than to `PROJECT_FORMAT_VERSION`, because the step from 1
    /// does change the model and this test is about the step that does not.
    #[test]
    fn the_version_zero_step_should_wrap_a_bare_model_in_the_envelope() {
        let bare = json!({ "frame_rate": 30.0 });

        let out = migrate(bare.clone(), UNVERSIONED, 1, STEPS).unwrap();

        assert_eq!(
            out.get(FIELD_VERSION).and_then(Value::as_u64),
            Some(1),
            "the envelope carries the version it was migrated to"
        );
        assert_eq!(
            out.get(FIELD_TIMELINE),
            Some(&bare),
            "the model is unchanged, only wrapped"
        );
    }

    /// Both steps, composed. The chain had one step until #1947, so this is the first
    /// test that two of them run in order on one document: the envelope goes on, and
    /// then the rate inside it becomes a ratio.
    #[test]
    fn migrate_should_run_both_steps_from_an_unversioned_document() {
        let bare = json!({ "frame_rate": 29.97, "canvas_width": 1920 });

        let out = migrate(bare, UNVERSIONED, PROJECT_FORMAT_VERSION, STEPS).unwrap();

        assert_eq!(
            out.get(FIELD_VERSION).and_then(Value::as_u64),
            Some(u64::from(PROJECT_FORMAT_VERSION)),
            "the last step stamps the version it produced"
        );
        let timeline = out.get(FIELD_TIMELINE).expect("the envelope went on");
        assert_eq!(
            timeline.get(FIELD_FRAME_RATE),
            Some(&rational_value(30000, 1001)),
            "and the rate inside it became the ratio 29.97 names"
        );
        assert_eq!(
            timeline.get("canvas_width").and_then(Value::as_u64),
            Some(1920),
            "the rest of the model is untouched"
        );
    }

    #[test]
    fn rate_to_rational_should_map_a_decimal_broadcast_rate_to_its_standard_rational() {
        for (decimal, num, den) in [
            (29.97, 30_000, 1001),
            (23.976, 24_000, 1001),
            (59.94, 60_000, 1001),
            (47.952, 48_000, 1001),
            (119.88, 120_000, 1001),
            // What an `f64` division of the exact ratio produces, which is what a host
            // that computed the rate rather than typing it would have stored.
            (30_000.0 / 1001.0, 30_000, 1001),
        ] {
            let document = json!({
                FIELD_VERSION: 1,
                FIELD_TIMELINE: { FIELD_FRAME_RATE: decimal },
            });
            let out = rate_to_rational(document).unwrap();
            assert_eq!(
                out.get(FIELD_TIMELINE)
                    .and_then(|m| m.get(FIELD_FRAME_RATE)),
                Some(&rational_value(num, den)),
                "{decimal} names {num}/{den}"
            );
        }
    }

    #[test]
    fn rate_to_rational_should_keep_an_integer_rate_exact() {
        for n in [24, 25, 30, 50, 60] {
            let document = json!({
                FIELD_VERSION: 1,
                FIELD_TIMELINE: { FIELD_FRAME_RATE: f64::from(n) },
            });
            let out = rate_to_rational(document).unwrap();
            assert_eq!(
                out.get(FIELD_TIMELINE)
                    .and_then(|m| m.get(FIELD_FRAME_RATE)),
                Some(&rational_value(n, 1)),
                "{n} fps is {n}/1, not a reduced thousandth"
            );
        }
    }

    #[test]
    fn rate_to_rational_should_reduce_an_unrecognised_rate() {
        // 12.5 names no standard rate, so it is read literally and reduced: 12500/1000
        // is 25/2.
        let document = json!({
            FIELD_VERSION: 1,
            FIELD_TIMELINE: { FIELD_FRAME_RATE: 12.5 },
        });
        let out = rate_to_rational(document).unwrap();
        assert_eq!(
            out.get(FIELD_TIMELINE)
                .and_then(|m| m.get(FIELD_FRAME_RATE)),
            Some(&rational_value(25, 2))
        );
    }

    /// The shape `rational_value` writes by hand has to be the shape `Rational`'s
    /// derived `Serialize` writes, or a migrated document would not deserialise into the
    /// model. Asserted against a real `Rational` rather than against another literal, so
    /// a change to the derive fails here instead of at a user's next load.
    #[test]
    fn the_migrations_ratio_shape_should_match_what_rational_serialises_to() {
        let fps = Rational::new(30000, 1001);
        let serialised: Value = serde_json::to_value(fps).expect("a Rational serialises");
        assert_eq!(serialised, rational_value(30000, 1001));
    }

    /// A bare model serialised by *this* release is read as version 0, because version 0
    /// is the absence of an envelope rather than a shape. The step from 0 wraps it and
    /// stamps 1, so this step is then handed a rate that is already a ratio and has to
    /// leave it alone. Found by `a_bare_serialised_timeline_should_load_as_version_zero`
    /// going red, not by the design pass.
    #[test]
    fn rate_to_rational_should_leave_a_rate_that_is_already_a_ratio() {
        let document = json!({
            FIELD_VERSION: 1,
            FIELD_TIMELINE: { FIELD_FRAME_RATE: rational_value(30000, 1001) },
        });
        let out = rate_to_rational(document).unwrap();
        assert_eq!(
            out.get(FIELD_TIMELINE)
                .and_then(|m| m.get(FIELD_FRAME_RATE)),
            Some(&rational_value(30000, 1001)),
            "an already-converted rate must survive unchanged, not be refused"
        );
        assert_eq!(out.get(FIELD_VERSION).and_then(Value::as_u64), Some(2));
    }

    #[test]
    fn rate_to_rational_should_stamp_version_two() {
        let document = json!({
            FIELD_VERSION: 1,
            FIELD_TIMELINE: { FIELD_FRAME_RATE: 30.0 },
        });
        let out = rate_to_rational(document).unwrap();
        assert_eq!(out.get(FIELD_VERSION).and_then(Value::as_u64), Some(2));
    }

    #[test]
    fn rate_to_rational_should_refuse_a_rate_that_is_not_a_rate() {
        for bad in [json!(0.0), json!(-30.0), json!(f64::MAX), json!("30")] {
            let document = json!({
                FIELD_VERSION: 1,
                FIELD_TIMELINE: { FIELD_FRAME_RATE: bad.clone() },
            });
            let err = rate_to_rational(document).unwrap_err();
            assert!(
                matches!(err, ProjectError::MigrationFailed { from, .. } if from == 1),
                "expected a failed migration out of version 1 for {bad}, got {err:?}"
            );
        }
    }

    /// An absent rate stays absent: the model's own `Deserialize` decides what a missing
    /// field means, and putting one in here would invent a rate the document never had.
    #[test]
    fn rate_to_rational_should_leave_an_absent_rate_absent() {
        let document = json!({
            FIELD_VERSION: 1,
            FIELD_TIMELINE: { "canvas_width": 1920 },
        });
        let out = rate_to_rational(document).unwrap();
        let timeline = out.get(FIELD_TIMELINE).expect("the envelope survives");
        assert!(timeline.get(FIELD_FRAME_RATE).is_none());
        assert_eq!(out.get(FIELD_VERSION).and_then(Value::as_u64), Some(2));
    }

    #[test]
    fn an_absent_format_version_should_read_as_zero() {
        assert_eq!(
            document_version(&json!({ "frame_rate": 30.0 })).unwrap(),
            UNVERSIONED
        );
        assert_eq!(document_version(&json!({ FIELD_VERSION: 7 })).unwrap(), 7);
    }

    /// A version that is present but unreadable is refused, naming the field. Reading it
    /// as 0 instead would wrap an envelope in another envelope and then report a fault in
    /// the model, which points the reader at the wrong thing.
    #[test]
    fn an_unreadable_format_version_should_be_refused_naming_the_field() {
        for bad in [json!("2"), json!(-1), json!(1.5), json!(null)] {
            let err = document_version(&json!({ FIELD_VERSION: bad.clone() })).unwrap_err();
            let ProjectError::Malformed { reason } = &err else {
                panic!("expected Malformed for format_version={bad}, got {err:?}");
            };
            assert!(
                reason.contains(FIELD_VERSION),
                "the reason must name the field, got {reason}"
            );
        }
    }

    #[test]
    fn timeline_from_should_refuse_an_envelope_with_no_model() {
        let err = timeline_from(&json!({ FIELD_VERSION: 1 })).unwrap_err();
        assert!(matches!(err, ProjectError::Malformed { .. }), "got {err:?}");
    }
}
