//! Shared low-level helper for reading decoded `AVFrame` plane rows.
//!
//! All `unsafe` for plane-pointer arithmetic is isolated here per the project's
//! unsafe-code convention (`*_inner.rs` files only).

#![allow(unsafe_code)]
#![allow(clippy::cast_possible_wrap)]

/// Returns the source pointer for row `y` of an `AVFrame` plane, matching
/// `FFmpeg`'s `av_image_copy` invariant `row_y = data + y * linesize` for
/// **both** scan directions.
///
/// `FFmpeg` signals bottom-up scan order with a **negative** `linesize`
/// (e.g. `vflip`-processed frames, some hardware decoders): `data` points at the
/// first byte of the *top* row (row 0), which sits at the *highest* address, and
/// each subsequent row lives at a *lower* address. The signed step
/// `data + y * linesize` yields the correct top-down row for both signs.
///
/// Casting `linesize` to `usize` first is wrong twice over (issue #1174): a
/// negative value wraps to a huge stride (out-of-bounds read), and stepping a
/// magnitude stride forward from the first row reverses row order (a vertical
/// flip). The signed offset here avoids both.
///
/// # Safety
///
/// `data` must be non-null and point to a plane whose row `y` is in bounds. The
/// plane occupies `[data + (rows-1)*linesize, data]` for a negative `linesize`
/// (i.e. `data` is the last, highest-address byte's row start), or
/// `[data, data + rows*linesize)` for a positive one.
#[inline]
#[must_use]
pub(crate) unsafe fn plane_row_ptr(data: *const u8, linesize: i32, y: usize) -> *const u8 {
    // SAFETY: the caller guarantees `data` is non-null and row `y` is within the
    // plane, so `data + y * linesize` stays in bounds for either scan direction.
    unsafe { data.offset(y as isize * linesize as isize) }
}

#[cfg(test)]
mod tests {
    use super::plane_row_ptr;

    #[test]
    fn positive_linesize_should_walk_rows_top_down() {
        let mem = [10u8, 20, 30];
        let data = mem.as_ptr();
        // SAFETY: rows 0..3 map to offsets 0,1,2 — all in bounds of `mem`.
        let got: Vec<u8> = (0..3)
            .map(|y| unsafe { *plane_row_ptr(data, 1, y) })
            .collect();
        assert_eq!(got, [10, 20, 30]);
    }

    #[test]
    fn negative_linesize_should_walk_rows_top_down_without_flip() {
        // Bottom-up frame (e.g. after `vflip`): `data` points at the top row,
        // which sits at the HIGHEST address; lower rows are at lower addresses.
        let mem = [10u8, 20, 30];
        // SAFETY: index 2 is the last element of `mem`.
        let data = unsafe { mem.as_ptr().add(2) };
        // Correct top-down order per `av_image_copy` (row_y = data + y*linesize)
        // is mem[2], mem[1], mem[0] == [30, 20, 10] — NOT the flipped [10,20,30].
        // SAFETY: rows 0..3 map to offsets 0,-1,-2 — all in bounds of `mem`.
        let got: Vec<u8> = (0..3)
            .map(|y| unsafe { *plane_row_ptr(data, -1, y) })
            .collect();
        assert_eq!(got, [30, 20, 10]);
    }

    #[test]
    fn row_zero_should_return_base_pointer() {
        let mem = [7u8; 4];
        let data = mem.as_ptr();
        // SAFETY: y=0 yields a zero offset regardless of linesize sign.
        assert_eq!(unsafe { plane_row_ptr(data, -13, 0) }, data);
    }
}
