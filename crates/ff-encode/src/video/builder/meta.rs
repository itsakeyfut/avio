//! Metadata, container, and miscellaneous settings for [`VideoEncoderBuilder`].

use super::VideoEncoderBuilder;
use crate::OutputContainer;

impl VideoEncoderBuilder {
    /// Set container format explicitly (usually auto-detected from file extension).
    #[must_use]
    pub fn container(mut self, container: OutputContainer) -> Self {
        self.container = Some(container);
        self
    }

    /// Mux into `sink` instead of writing a file.
    ///
    /// The path given to [`VideoEncoder::create`](crate::VideoEncoder::create) is
    /// then only what the muxer is guessed from -- nothing is created on disk --
    /// so it still needs a usable extension, or an explicit
    /// [`container`](Self::container).
    ///
    /// `sink` is anything that writes and seeks and can move to the encoder's
    /// thread. Seeking is not optional: MP4 rewrites its header once the sizes
    /// are known, and a sink that cannot seek would produce an unplayable file.
    ///
    /// Incompatible with [`two_pass`](Self::two_pass), which opens the output
    /// again for the second pass; that combination is rejected by `build`.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use std::io::Cursor;
    /// use ff_encode::VideoEncoder;
    ///
    /// let mut encoder = VideoEncoder::create("out.mp4")
    ///     .video_size(640, 360)
    ///     .output_sink(Cursor::new(Vec::new()))
    ///     .build()?;
    /// # Ok::<(), ff_encode::EncodeError>(())
    /// ```
    #[must_use]
    pub fn output_sink(mut self, sink: impl ff_sys::IoSink + 'static) -> Self {
        self.sink = Some(Box::new(sink));
        self
    }

    /// Set a closure as the progress callback.
    #[must_use]
    pub fn on_progress<F>(mut self, callback: F) -> Self
    where
        F: FnMut(&crate::EncodeProgress) + Send + 'static,
    {
        self.progress_callback = Some(Box::new(callback));
        self
    }

    /// Set a [`crate::EncodeProgressCallback`] trait object (supports cancellation).
    #[must_use]
    pub fn progress_callback<C: crate::EncodeProgressCallback + 'static>(
        mut self,
        callback: C,
    ) -> Self {
        self.progress_callback = Some(Box::new(callback));
        self
    }

    /// Enable two-pass encoding for more accurate bitrate distribution.
    ///
    /// Two-pass encoding is video-only and is incompatible with audio streams.
    #[must_use]
    pub fn two_pass(mut self) -> Self {
        self.two_pass = true;
        self
    }

    /// Relocate the `moov` atom to the front of MP4/MOV output (`movflags=+faststart`).
    ///
    /// This makes the file playable via progressive download / streaming before it
    /// is fully fetched, the standard requirement for web-delivered files. It has no
    /// effect on non-MP4/MOV containers or on fragmented MP4 (which already streams
    /// via its own movflags). Because `FFmpeg` relocates the atom by rewriting the
    /// file at finalize, faststart adds a second pass over the output, so it is not
    /// free for very large files.
    #[must_use]
    pub fn faststart(mut self) -> Self {
        self.faststart = true;
        self
    }

    /// Embed a metadata tag in the output container.
    ///
    /// Calls `av_dict_set` on `AVFormatContext->metadata` before the header
    /// is written. Multiple calls accumulate entries; duplicate keys use the
    /// last value.
    #[must_use]
    pub fn metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.push((key.to_string(), value.to_string()));
        self
    }

    /// Add a chapter to the output container.
    ///
    /// Allocates an `AVChapter` entry on `AVFormatContext` before the header
    /// is written. Multiple calls accumulate chapters in the order added.
    #[must_use]
    pub fn chapter(mut self, chapter: ff_format::chapter::ChapterInfo) -> Self {
        self.chapters.push(chapter);
        self
    }

    /// Copy a subtitle stream from an existing file into the output container.
    ///
    /// Opens `source_path`, locates the stream at `stream_index`, and registers it
    /// as a passthrough stream in the output.  Packets are copied verbatim using
    /// `av_interleaved_write_frame` without re-encoding.
    ///
    /// `stream_index` is the zero-based index of the subtitle stream inside
    /// `source_path`.  For files with a single subtitle track this is typically `0`
    /// (or whichever index `ffprobe` reports).
    ///
    /// If the source cannot be opened or the stream index is invalid, a warning is
    /// logged and encoding continues without subtitles.
    #[must_use]
    pub fn subtitle_passthrough(mut self, source_path: &str, stream_index: usize) -> Self {
        self.subtitle_passthrough = Some((source_path.to_string(), stream_index));
        self
    }

    /// Set a codec-*private* option by name, for the long tail that has no typed
    /// builder (`x264-params`, `aq-mode`, `psy-rd`, ...).
    ///
    /// Applies to the **video** codec only. A video output's audio track opens
    /// its own codec context, which this does not reach; use
    /// [`AudioEncoder`](crate::AudioEncoder) for audio-only output that needs
    /// the same escape hatch.
    ///
    /// Repeatable, and applied in call order via `av_opt_set` on the codec's
    /// `priv_data` before `avcodec_open2`, **after**
    /// [`codec_options()`](Self::codec_options) — so a key named here overrides
    /// the same key set through the typed API.
    ///
    /// # Escape-hatch semantics
    ///
    /// Prefer [`codec_options()`](Self::codec_options): it is validated at
    /// compile time and portable across encoders. Nothing here is checked until
    /// `FFmpeg` sees it, and keys are codec-specific.
    ///
    /// Unlike the typed options, which log and continue when an encoder does not
    /// support them, an option rejected here fails
    /// [`build()`](Self::build) with [`crate::EncodeError::InvalidConfig`] — the
    /// key was named explicitly, so dropping it silently would defeat the
    /// purpose. The consequence is worth planning for: a configuration carrying
    /// libx264 keys will fail once the caller switches to a hardware encoder.
    ///
    /// ```no_run
    /// # use ff_encode::VideoEncoder;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let encoder = VideoEncoder::create("out.mp4")
    ///     .video(1920, 1080, 30.0)
    ///     .codec_opt("x264-params", "keyint=48:min-keyint=48")
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn codec_opt(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.codec_opts.push((key.into(), value.into()));
        self
    }

    /// Set per-codec encoding options.
    ///
    /// Applied via `av_opt_set` before `avcodec_open2` during [`build()`](Self::build).
    /// This is additive — omitting it leaves codec defaults unchanged.
    /// Any option that the chosen encoder does not support is logged as a
    /// warning and skipped; it never causes `build()` to return an error.
    ///
    /// The [`crate::VideoCodecOptions`] variant should match the codec selected via
    /// [`video_codec()`](Self::video_codec).  A mismatch is silently ignored.
    #[must_use]
    pub fn codec_options(mut self, opts: crate::VideoCodecOptions) -> Self {
        self.codec_options = Some(opts);
        self
    }

    /// Embed a binary attachment in the output container.
    ///
    /// Attachments are supported in MKV/WebM containers and are used for
    /// fonts (required by ASS/SSA subtitle rendering), cover art, or other
    /// binary files that consumers of the file may need.
    ///
    /// - `data` — raw bytes of the attachment
    /// - `mime_type` — MIME type string (e.g. `"application/x-truetype-font"`,
    ///   `"image/jpeg"`)
    /// - `filename` — the name reported inside the container (e.g. `"Arial.ttf"`)
    ///
    /// Multiple calls accumulate entries; each attachment becomes its own stream
    /// with `AVMEDIA_TYPE_ATTACHMENT` codec parameters.
    #[must_use]
    pub fn add_attachment(mut self, data: Vec<u8>, mime_type: &str, filename: &str) -> Self {
        self.attachments
            .push((data, mime_type.to_string(), filename.to_string()));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn builder_container_should_be_stored() {
        let builder = VideoEncoderBuilder::new(PathBuf::from("output.mp4"))
            .video(1920, 1080, 30.0)
            .container(OutputContainer::Mp4);
        assert_eq!(builder.container, Some(OutputContainer::Mp4));
    }

    #[test]
    fn two_pass_flag_should_be_stored_in_builder() {
        let builder = VideoEncoderBuilder::new(PathBuf::from("output.mp4"))
            .video(640, 480, 30.0)
            .two_pass();
        assert!(builder.two_pass);
    }

    #[test]
    fn faststart_flag_should_be_stored_in_builder() {
        let builder = VideoEncoderBuilder::new(PathBuf::from("output.mp4"))
            .video(640, 480, 30.0)
            .faststart();
        assert!(builder.faststart);
    }

    #[test]
    fn add_attachment_should_accumulate_entries() {
        let builder = VideoEncoderBuilder::new(PathBuf::from("output.mkv"))
            .video(320, 240, 30.0)
            .add_attachment(vec![1, 2, 3], "application/x-truetype-font", "font.ttf")
            .add_attachment(vec![4, 5, 6], "image/jpeg", "cover.jpg");
        assert_eq!(builder.attachments.len(), 2);
        assert_eq!(builder.attachments[0].0, vec![1u8, 2, 3]);
        assert_eq!(builder.attachments[0].1, "application/x-truetype-font");
        assert_eq!(builder.attachments[0].2, "font.ttf");
        assert_eq!(builder.attachments[1].1, "image/jpeg");
        assert_eq!(builder.attachments[1].2, "cover.jpg");
    }

    #[test]
    fn add_attachment_with_no_attachments_should_start_empty() {
        let builder = VideoEncoderBuilder::new(PathBuf::from("output.mkv")).video(320, 240, 30.0);
        assert!(builder.attachments.is_empty());
    }
}
