//! Type-erased columns: one contiguous, aligned block of rows for a single component.

use alloc::alloc::{alloc, dealloc, handle_alloc_error, realloc};
use core::alloc::Layout;
use core::cmp::max;
use core::num::NonZeroUsize;
use core::ptr::NonNull;

/// Storage for every row of one component inside one table.
pub struct Column {
    /// Base of the row buffer
    ptr: NonNull<u8>,
    /// Number of initialized rows, at the front of the buffer.
    len: usize,
    /// Number of rows the buffer can hold before it must grow. `usize::MAX` for tag columns.
    capacity: usize,
    /// Size and alignment of one row, as computed by the component's row layout.
    item_layout: Layout,
}

impl Column {
    /// Creates an empty column without allocating.
    ///
    /// # Arguments
    ///
    /// * `layout` - Size and alignment of one row; a size of 0 makes a tag column
    ///
    /// # Returns
    ///
    /// A column with `len() == 0`. Its capacity is 0, or `usize::MAX` for a tag column, since
    /// zero-sized rows never need memory.
    pub fn new(layout: Layout) -> Self {
        let capacity = if layout.size() == 0 { usize::MAX } else { 0 };
        Self {
            ptr: NonNull::without_provenance(
                NonZeroUsize::new(layout.align()).expect("Alignment is never zero"),
            ),
            len: 0,
            capacity,
            item_layout: layout,
        }
    }

    /// Returns the number of rows stored in the column.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the column stores no rows.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the number of rows the column can hold without reallocating.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Ensures room for at least `additional` more rows beyond `len`.
    ///
    /// # Arguments
    ///
    /// * `additional` - Number of rows the caller is about to add
    ///
    /// # Panics
    ///
    /// Panics with "capacity overflow" if the buffer size would not fit in `isize::MAX` bytes.
    pub fn reserve(&mut self, additional: usize) {
        let required = self.len.checked_add(additional).expect("capacity overflow");
        if self.capacity >= required {
            return;
        }
        let new_capacity = max(required, self.capacity.saturating_mul(2));
        let new_layout = self.buffer_layout(new_capacity).expect("capacity overflow");
        if self.capacity == 0 {
            // SAFETY: `new_layout` is non-zero-size: tags returned early and `new_capacity >= 1`.
            // `new_unchecked` follows the null check.
            unsafe {
                let ptr = alloc(new_layout);
                if ptr.is_null() {
                    handle_alloc_error(new_layout);
                }
                self.ptr = NonNull::new_unchecked(ptr);
                self.capacity = new_capacity
            }
        } else {
            // SAFETY: `self.ptr` was allocated with `buffer_layout(self.capacity)`; the new size
            // is non-zero and valid. `new_unchecked` follows the null check.
            unsafe {
                let ptr = realloc(
                    self.ptr.as_ptr(),
                    self.buffer_layout(self.capacity)
                        .expect("buffer layout was valid when allocated"),
                    new_layout.size(),
                );
                if ptr.is_null() {
                    handle_alloc_error(new_layout);
                }
                self.ptr = NonNull::new_unchecked(ptr);
                self.capacity = new_capacity
            }
        }
    }

    /// Returns the layout of a buffer holding `capacity` rows.
    ///
    /// Every allocation, reallocation and deallocation goes through this function, so the
    /// layout passed to `dealloc` and `realloc` always matches the one used to allocate.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Number of rows the buffer holds
    ///
    /// # Returns
    ///
    /// `None` if the total size overflows `usize` or exceeds `isize::MAX` once aligned.
    fn buffer_layout(&self, capacity: usize) -> Option<Layout> {
        let size = self.item_layout.size().checked_mul(capacity)?;
        Layout::from_size_align(size, self.item_layout.align()).ok()
    }
}
impl Drop for Column {
    fn drop(&mut self) {
        if self.item_layout.size() != 0 && self.capacity != 0 {
            // SAFETY: non-zero row size and capacity means `reserve` allocated `self.ptr` with
            // `buffer_layout(self.capacity)`.
            unsafe {
                dealloc(
                    self.ptr.as_ptr(),
                    self.buffer_layout(self.capacity)
                        .expect("buffer layout was valid when allocated"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /* AI GENERATED TEST */
    #[test]
    fn new_column_is_empty_without_allocating() {
        let layout = Layout::from_size_align(16, 8).expect("valid layout");

        let column = Column::new(layout);

        assert_eq!(column.len(), 0);
        assert!(column.is_empty());
        assert_eq!(column.capacity(), 0);
    }

    /* AI GENERATED TEST */
    #[test]
    fn new_column_pointer_is_aligned_to_row_alignment() {
        for align in [1, 2, 4, 8, 16] {
            let layout = Layout::from_size_align(align, align).expect("valid layout");

            let column = Column::new(layout);

            assert_eq!(
                column.ptr.as_ptr().addr() % align,
                0,
                "dangling pointer must be {align}-aligned"
            );
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn new_tag_column_pointer_is_aligned() {
        // A tag has no fields: size 0, align 1. Its column never allocates, so the dangling
        // pointer is the base pointer forever and must still be valid for zero-byte access.
        let layout = Layout::from_size_align(0, 1).expect("valid layout");

        let column = Column::new(layout);

        assert!(column.is_empty());
        assert_eq!(column.ptr.as_ptr().addr() % layout.align(), 0);
    }

    /* AI GENERATED TEST */
    #[test]
    fn reserve_allocates_aligned_memory() {
        for align in [1, 2, 4, 8, 16, 64] {
            let layout = Layout::from_size_align(align, align).expect("valid layout");
            let mut column = Column::new(layout);

            column.reserve(3);

            assert!(column.capacity() >= 3);
            assert_eq!(
                column.ptr.as_ptr().addr() % align,
                0,
                "allocation must be {align}-aligned"
            );
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn reserve_within_capacity_does_not_reallocate() {
        let layout = Layout::from_size_align(16, 8).expect("valid layout");
        let mut column = Column::new(layout);
        column.reserve(8);
        let ptr_before = column.ptr;
        let capacity_before = column.capacity();

        column.reserve(capacity_before);

        assert_eq!(column.ptr, ptr_before);
        assert_eq!(column.capacity(), capacity_before);
    }

    /* AI GENERATED TEST */
    #[test]
    fn growing_doubles_capacity_and_preserves_bytes() {
        let row_size = 8;
        let layout = Layout::from_size_align(row_size, 8).expect("valid layout");
        let mut column = Column::new(layout);
        column.reserve(4);
        let capacity_before = column.capacity();
        let byte_count = capacity_before * row_size;
        // SAFETY: `reserve` allocated `capacity_before` rows of `row_size` bytes each.
        for i in 0..byte_count {
            unsafe { column.ptr.as_ptr().add(i).write(i as u8) };
        }
        // Pretend every row was pushed, so the next reserve has to grow.
        column.len = capacity_before;

        column.reserve(1);

        assert!(column.capacity() >= 2 * capacity_before);
        for i in 0..byte_count {
            // SAFETY: growth keeps the first `byte_count` bytes, now inside a larger allocation.
            let byte = unsafe { column.ptr.as_ptr().add(i).read() };
            assert_eq!(byte, i as u8, "byte {i} lost during growth");
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn growth_doubles_capacity_even_when_partly_full() {
        let layout = Layout::from_size_align(16, 8).expect("valid layout");
        let mut column = Column::new(layout);
        column.reserve(8);
        let capacity_before = column.capacity();
        // One row in use, so `len` and `capacity` differ when growth happens.
        column.len = 1;

        column.reserve(capacity_before);

        assert!(
            column.capacity() >= 2 * capacity_before,
            "capacity {} did not double from {capacity_before}",
            column.capacity()
        );
    }

    /* AI GENERATED TEST */
    #[test]
    #[should_panic(expected = "capacity overflow")]
    fn reserve_panics_when_row_count_overflows() {
        let layout = Layout::from_size_align(16, 8).expect("valid layout");
        let mut column = Column::new(layout);
        column.reserve(1);
        column.len = 1;

        // `len + additional` overflows usize before any byte size is computed.
        column.reserve(usize::MAX);
    }

    /* AI GENERATED TEST */
    #[test]
    fn tag_column_never_allocates() {
        let layout = Layout::from_size_align(0, 1).expect("valid layout");
        let mut column = Column::new(layout);
        let ptr_before = column.ptr;

        column.reserve(1_000_000);

        assert_eq!(column.capacity(), usize::MAX);
        assert_eq!(column.ptr, ptr_before);
    }

    /* AI GENERATED TEST */
    #[test]
    #[should_panic(expected = "capacity overflow")]
    fn reserve_panics_on_capacity_overflow() {
        let layout = Layout::from_size_align(16, 8).expect("valid layout");
        let mut column = Column::new(layout);

        // 16 bytes per row times usize::MAX / 8 rows overflows usize.
        column.reserve(usize::MAX / 8);
    }
}
