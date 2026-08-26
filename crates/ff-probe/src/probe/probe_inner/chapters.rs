//! Chapter extraction.

use ff_format::Rational;
use ff_format::chapter::ChapterInfo;

use super::mapping::pts_to_duration;

/// Extracts all chapters from the demux context.
pub(super) fn extract_chapters(ctx: &ff_sys::InputFormatContext) -> Vec<ChapterInfo> {
    ctx.chapters().map(extract_single_chapter).collect()
}

/// Extracts information from a single chapter handle.
fn extract_single_chapter(chapter: ff_sys::ChapterRef<'_>) -> ChapterInfo {
    let id = chapter.id();

    let av_tb = chapter.time_base();
    let time_base = if av_tb.den != 0 {
        Some(Rational::new(av_tb.num, av_tb.den))
    } else {
        log::warn!(
            "chapter time_base has zero denominator, treating as unknown \
             chapter_id={id} time_base_num={num} time_base_den=0",
            num = av_tb.num
        );
        None
    };

    let (start, end) = if let Some(tb) = time_base {
        (
            pts_to_duration(chapter.start(), tb),
            pts_to_duration(chapter.end(), tb),
        )
    } else {
        (std::time::Duration::ZERO, std::time::Duration::ZERO)
    };

    // Pull the chapter tags once; the "title" tag is surfaced separately and the
    // remaining tags become the chapter's metadata map.
    let mut tags = chapter.metadata();
    let title = tags.remove("title");

    let mut builder = ChapterInfo::builder().id(id).start(start).end(end);

    if let Some(t) = title {
        builder = builder.title(t);
    }
    if let Some(tb) = time_base {
        builder = builder.time_base(tb);
    }
    if !tags.is_empty() {
        builder = builder.metadata(tags);
    }

    builder.build()
}
