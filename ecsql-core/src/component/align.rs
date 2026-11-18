//! Alignment arithmetic and the physical row layout of a component.

use alloc::vec;
use alloc::vec::Vec;

use super::FieldType;

/// Rounds `value` up to the next multiple of `align`.
///
/// # Arguments
///
/// * `value` - Byte offset or size to round up
/// * `align` - Target alignment; must be a power of two
///
/// # Returns
///
/// The smallest multiple of `align` that is `>= value`.
pub const fn align_up(value: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

/// Physical placement of a component's fields inside one row of its column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RowLayout {
    /// Byte offset of each field, in declaration order (not physical order).
    pub(super) offsets: Vec<usize>,
    /// Bytes per row, including tail padding: the column's stride.
    pub(super) size: usize,
    /// Alignment of the whole row: the largest field alignment, or 1 for a tag.
    pub(super) align: usize,
}

impl RowLayout {
    /// Places fields in descending alignment order to minimise padding.
    ///
    /// Fields with equal alignment keep their declaration order, so the same definition always
    /// produces the same layout.
    ///
    /// # Arguments
    ///
    /// * `types` - Field types in declaration order
    pub(super) fn compute(types: &[FieldType]) -> Self {
        let mut orders = (0..types.len()).collect::<Vec<_>>();
        // stable sort, so equal alignments keep declaration order."
        orders.sort_by_key(|i| core::cmp::Reverse(types[*i].align()));
        // marker struct are aligned of ()
        let largest_align = types
            .iter()
            .map(|f| f.align())
            .max()
            .unwrap_or(align_of::<()>());
        let mut offsets = vec![0usize; orders.len()];
        let mut size = 0;
        for i in orders {
            size = align_up(size, types[i].align());
            offsets[i] = size;
            size += types[i].size();
        }
        size = align_up(size, largest_align);
        Self {
            offsets,
            size,
            align: largest_align,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_up_rounds_to_power_of_two_multiples() {
        let cases = [
            (0, 8, 0),
            (1, 8, 8),
            (8, 8, 8),
            (9, 4, 12),
            (5, 1, 5),
            (17, 16, 32),
        ];

        for (value, align, expected) in cases {
            assert_eq!(
                align_up(value, align),
                expected,
                "align_up({value}, {align})"
            );
        }
    }

    #[test]
    fn layout_of_tag_is_empty() {
        let layout = RowLayout::compute(&[]);

        assert_eq!(
            layout,
            RowLayout {
                offsets: Vec::new(),
                size: 0,
                align: 1
            }
        );
    }

    #[test]
    fn layout_of_same_alignment_fields_keeps_declaration_order() {
        use FieldType::{F32, I32, U32};

        let layout = RowLayout::compute(&[I32, F32, U32]);

        assert_eq!(
            layout,
            RowLayout {
                offsets: alloc::vec![0, 4, 8],
                size: 12,
                align: 4
            }
        );
    }

    #[test]
    fn layout_reorders_fields_to_remove_padding() {
        use FieldType::{F64, U8, U16};

        // Declared (a u8, b f64, c u16). In declaration order this would need 7 bytes of padding
        // after `a` and 6 at the tail: 24 bytes. Sorted by alignment it is b, c, a: 16 bytes.
        let layout = RowLayout::compute(&[U8, F64, U16]);

        assert_eq!(
            layout,
            RowLayout {
                offsets: alloc::vec![10, 0, 8],
                size: 16,
                align: 8
            }
        );
    }

    #[test]
    fn layout_adds_tail_padding_so_rows_stay_aligned() {
        use FieldType::{Bool, Entity};

        // Without tail padding the next row's `target` would start at offset 9, misaligned.
        let layout = RowLayout::compute(&[Bool, Entity]);

        assert_eq!(
            layout,
            RowLayout {
                offsets: alloc::vec![8, 0],
                size: 16,
                align: 8
            }
        );
    }

    #[test]
    fn layout_matches_core_alloc_layout() {
        use FieldType::{Bool, F32, F64, I16, U8};
        use core::alloc::Layout;

        let types = [U8, F64, I16, Bool, F32];
        let ours = RowLayout::compute(&types);

        // Build the same row with the standard library: place fields in our physical order
        // using `Layout::extend`, which implements the C struct layout rules.
        let mut physical: Vec<(usize, FieldType)> = types.iter().copied().enumerate().collect();
        physical.sort_by_key(|&(i, _)| ours.offsets[i]);
        let mut reference = Layout::from_size_align(0, 1).expect("valid layout");
        for (i, ty) in physical {
            let field = Layout::from_size_align(ty.size(), ty.align()).expect("valid layout");
            let (extended, offset) = reference.extend(field).expect("no overflow");
            assert_eq!(offset, ours.offsets[i], "offset of field {i}");
            reference = extended;
        }
        let reference = reference.pad_to_align();

        assert_eq!(
            (ours.size, ours.align),
            (reference.size(), reference.align())
        );
    }
}
