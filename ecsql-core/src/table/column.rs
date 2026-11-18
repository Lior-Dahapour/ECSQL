//! Type-erased column: one contiguous, aligned block of rows for a single component.

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

    /// Returns the size and alignment of one row.
    pub fn item_layout(&self) -> Layout {
        self.item_layout
    }

    /// Appends one row, copied from `row`.
    ///
    /// # Panics
    ///
    /// Panics with "row size mismatch" if `row.len()` is not the row size.
    pub fn push(&mut self, row: &[u8]) {
        assert_eq!(row.len(), self.item_layout.size(), "row size mismatch");
        self.reserve(1);
        // SAFETY: `reserve` made `len < capacity`, so row `len` is in bounds; `row` is a separate
        // borrow, so it cannot overlap the column's buffer.
        unsafe { core::ptr::copy_nonoverlapping(row.as_ptr(), self.row_ptr(self.len), row.len()) };
        self.len += 1;
    }

    /// Returns the bytes of row `index`, or `None` if it is out of bounds.
    pub fn get(&self, index: usize) -> Option<&[u8]> {
        if index >= self.len {
            return None;
        }
        // SAFETY: `index < len <= capacity`, so the row is in bounds and initialized.
        Some(unsafe { core::slice::from_raw_parts(self.row_ptr(index), self.item_layout.size()) })
    }

    /// Returns the bytes of row `index` for writing, or `None` if it is out of bounds.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut [u8]> {
        if index >= self.len {
            return None;
        }
        // SAFETY: `index < len <= capacity`, so the row is in bounds and initialized.
        // and &mut self makes this the only reference into the column.
        Some(unsafe {
            core::slice::from_raw_parts_mut(self.row_ptr(index), self.item_layout.size())
        })
    }

    /// Removes row `index` in O(1) by moving the last row into its place.
    ///
    /// # Panics
    ///
    /// Panics with "swap_remove index out of bounds" if `index >= len`.
    pub fn swap_remove(&mut self, index: usize) {
        assert!(index < self.len, "swap_remove index out of bounds");
        let last = self.len() - 1;
        if index != last {
            // SAFETY: `index < last < len`, so both rows are in bounds and initialized, and they are
            // distinct, so they don't overlap.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.row_ptr(last),
                    self.row_ptr(index),
                    self.item_layout.size(),
                );
            }
        }
        self.len -= 1;
    }

    /// Moves row `index` to the end of `dst`, then swap-removes it from this column.
    ///
    /// # Panics
    ///
    /// Panics with "column layouts differ" if `dst` stores a different row layout, and with
    /// "move_row_to index out of bounds" if `index >= len`.
    pub fn move_row_to(&mut self, index: usize, dst: &mut Column) {
        assert_eq!(self.item_layout, dst.item_layout, "column layouts differ");
        let row = self.get(index).expect("move_row_to index out of bounds");
        dst.push(row);
        self.swap_remove(index);
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

    /// Returns a pointer to the first byte of row `index`.
    ///
    /// # Safety
    ///
    /// `index` must be `<= self.capacity`, so the pointer stays inside the buffer or one past it.
    unsafe fn row_ptr(&self, index: usize) -> *mut u8 {
        // SAFETY: the caller guarantees `index <= capacity`, so the offset stays within the buffer
        // or one past it, and `index * size` cannot overflow since `buffer_layout` checked it.
        unsafe { self.ptr.as_ptr().add(index * self.item_layout.size()) }
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

    /// A 16-byte, 8-aligned row whose every byte is `seed + offset`, so rows are distinct.
    fn sample_row(seed: u8) -> [u8; 16] {
        core::array::from_fn(|offset| seed.wrapping_add(offset as u8))
    }

    fn sixteen_byte_column() -> Column {
        Column::new(Layout::from_size_align(16, 8).expect("valid layout"))
    }

    /* AI GENERATED TEST */
    #[test]
    fn push_then_get_round_trips_bytes() {
        let mut column = sixteen_byte_column();
        let rows = [sample_row(0), sample_row(100), sample_row(200)];

        for row in &rows {
            column.push(row);
        }

        assert_eq!(column.len(), 3);
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(column.get(index), Some(&row[..]), "row {index}");
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn get_out_of_bounds_is_none() {
        let mut column = sixteen_byte_column();
        column.push(&sample_row(0));
        column.reserve(10);

        // Index 1 is inside the allocation but not an initialized row.
        assert_eq!(column.get(1), None);
        assert_eq!(column.get(usize::MAX), None);
        assert_eq!(column.get_mut(1), None);
    }

    /* AI GENERATED TEST */
    #[test]
    fn push_grows_and_keeps_every_row() {
        let mut column = sixteen_byte_column();

        for seed in 0..100 {
            column.push(&sample_row(seed));
        }

        assert_eq!(column.len(), 100);
        for seed in 0..100 {
            assert_eq!(column.get(usize::from(seed)), Some(&sample_row(seed)[..]));
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn rows_are_aligned_and_one_stride_apart() {
        let mut column = sixteen_byte_column();
        column.push(&sample_row(0));
        column.push(&sample_row(1));

        let first = column.get(0).expect("row 0").as_ptr().addr();
        let second = column.get(1).expect("row 1").as_ptr().addr();

        assert_eq!(first % 8, 0);
        assert_eq!(second - first, 16);
    }

    /* AI GENERATED TEST */
    #[test]
    fn get_mut_writes_through_to_the_row() {
        let mut column = sixteen_byte_column();
        column.push(&sample_row(0));
        column.push(&sample_row(1));

        column
            .get_mut(1)
            .expect("row 1")
            .copy_from_slice(&sample_row(50));

        assert_eq!(column.get(0), Some(&sample_row(0)[..]));
        assert_eq!(column.get(1), Some(&sample_row(50)[..]));
    }

    /* AI GENERATED TEST */
    #[test]
    #[should_panic(expected = "row size mismatch")]
    fn push_rejects_wrong_row_size() {
        let mut column = sixteen_byte_column();

        column.push(&[0; 8]);
    }

    /* AI GENERATED TEST */
    #[test]
    fn tag_column_push_counts_rows_without_allocating() {
        let mut column = Column::new(Layout::from_size_align(0, 1).expect("valid layout"));
        let ptr_before = column.ptr;

        for _ in 0..1000 {
            column.push(&[]);
        }

        assert_eq!(column.len(), 1000);
        assert_eq!(column.get(999), Some(&[][..]));
        assert_eq!(column.get(1000), None);
        assert_eq!(column.ptr, ptr_before);
    }

    /// A 16-byte column holding one `sample_row` per seed, in order.
    fn column_with_rows(seeds: &[u8]) -> Column {
        let mut column = sixteen_byte_column();
        for &seed in seeds {
            column.push(&sample_row(seed));
        }
        column
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_moves_last_row_into_the_hole() {
        let mut column = column_with_rows(&[0, 10, 20, 30]);

        column.swap_remove(1);

        assert_eq!(column.len(), 3);
        assert_eq!(column.get(0), Some(&sample_row(0)[..]));
        assert_eq!(column.get(1), Some(&sample_row(30)[..]));
        assert_eq!(column.get(2), Some(&sample_row(20)[..]));
        assert_eq!(column.get(3), None);
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_last_row_only_shrinks() {
        let mut column = column_with_rows(&[0, 10, 20]);

        column.swap_remove(2);

        assert_eq!(column.len(), 2);
        assert_eq!(column.get(0), Some(&sample_row(0)[..]));
        assert_eq!(column.get(1), Some(&sample_row(10)[..]));
        assert_eq!(column.get(2), None);
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_keeps_memory_for_reuse() {
        let mut column = column_with_rows(&[0]);
        let ptr_before = column.ptr;
        let capacity_before = column.capacity();

        column.swap_remove(0);
        column.push(&sample_row(99));

        assert_eq!(column.len(), 1);
        assert_eq!(column.get(0), Some(&sample_row(99)[..]));
        assert_eq!(column.ptr, ptr_before);
        assert_eq!(column.capacity(), capacity_before);
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_every_row_from_the_front_empties_the_column() {
        let mut column = column_with_rows(&[0, 10, 20, 30, 40]);

        // Each removal at 0 pulls the current last row to the front.
        let expected_front = [40, 30, 20, 10];
        for seed in expected_front {
            column.swap_remove(0);
            assert_eq!(column.get(0), Some(&sample_row(seed)[..]));
        }
        column.swap_remove(0);

        assert!(column.is_empty());
    }

    /* AI GENERATED TEST */
    #[test]
    #[should_panic(expected = "swap_remove index out of bounds")]
    fn swap_remove_panics_out_of_bounds() {
        let mut column = column_with_rows(&[0, 10]);

        column.swap_remove(2);
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_on_tag_column_only_counts() {
        let mut column = Column::new(Layout::from_size_align(0, 1).expect("valid layout"));
        for _ in 0..5 {
            column.push(&[]);
        }

        column.swap_remove(2);

        assert_eq!(column.len(), 4);
        assert_eq!(column.get(3), Some(&[][..]));
        assert_eq!(column.get(4), None);
    }

    /* AI GENERATED TEST */
    #[test]
    fn move_row_to_appends_to_destination_and_swap_removes_source() {
        let mut src = column_with_rows(&[0, 10, 20]);
        let mut dst = column_with_rows(&[99]);

        src.move_row_to(0, &mut dst);

        assert_eq!(dst.len(), 2);
        assert_eq!(dst.get(0), Some(&sample_row(99)[..]));
        assert_eq!(dst.get(1), Some(&sample_row(0)[..]));
        assert_eq!(src.len(), 2);
        assert_eq!(src.get(0), Some(&sample_row(20)[..]));
        assert_eq!(src.get(1), Some(&sample_row(10)[..]));
    }

    /* AI GENERATED TEST */
    #[test]
    fn move_row_to_last_row_leaves_other_rows_in_place() {
        let mut src = column_with_rows(&[0, 10]);
        let mut dst = column_with_rows(&[]);

        src.move_row_to(1, &mut dst);

        assert_eq!(dst.get(0), Some(&sample_row(10)[..]));
        assert_eq!(src.len(), 1);
        assert_eq!(src.get(0), Some(&sample_row(0)[..]));
    }

    /* AI GENERATED TEST */
    #[test]
    fn move_row_to_grows_destination() {
        let seeds: [u8; 50] = core::array::from_fn(|i| i as u8);
        let mut src = column_with_rows(&seeds);
        let mut dst = column_with_rows(&[]);

        while !src.is_empty() {
            src.move_row_to(0, &mut dst);
        }

        assert_eq!(dst.len(), 50);
        assert!(dst.capacity() >= 50);
    }

    /* AI GENERATED TEST */
    #[test]
    #[should_panic(expected = "move_row_to index out of bounds")]
    fn move_row_to_panics_out_of_bounds() {
        let mut src = column_with_rows(&[0]);
        let mut dst = column_with_rows(&[]);

        src.move_row_to(1, &mut dst);
    }

    /* AI GENERATED TEST */
    #[test]
    #[should_panic(expected = "column layouts differ")]
    fn move_row_to_panics_on_layout_mismatch() {
        let mut src = column_with_rows(&[0]);
        // Same size, different alignment: a different component, so moving is a bug.
        let mut dst = Column::new(Layout::from_size_align(16, 4).expect("valid layout"));

        src.move_row_to(0, &mut dst);
    }

    /* AI GENERATED TEST */
    #[test]
    fn move_row_to_between_tag_columns_only_counts() {
        let tag = Layout::from_size_align(0, 1).expect("valid layout");
        let mut src = Column::new(tag);
        let mut dst = Column::new(tag);
        for _ in 0..3 {
            src.push(&[]);
        }

        src.move_row_to(1, &mut dst);

        assert_eq!(src.len(), 2);
        assert_eq!(dst.len(), 1);
    }
}
