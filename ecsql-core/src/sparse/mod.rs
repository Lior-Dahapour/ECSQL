//! Sparse component storage: components addressed directly by entity index.
//!
//! A `SPARSE` component lives outside the archetype tables, so adding or removing it never
//! moves the entity: it sets or clears one presence bit and one data slot.
//!
//! ```text
//! presence page   [header 24 B | 1,021 x u64 words]          bit i = entity index i, 1 = present
//!                 index i -> page i / 65,344, word (i % 65,344) / 64, bit i % 64
//!
//! data page       [header 24 B | slot 0 | slot 1 | ... | slot S-1 | unused]
//!                 S = (8192 - 24) / size
//!                 index i -> page i / S, slot i % S, byte 24 + (i % S) * size
//! ```
//!
//! - In memory, presence is one flat word array; 65,344 = 1,021 x 64, so it splits into pages
//!   with no word straddling two.
//! - A data page exists only while one of its slots is present (header `row_count` = live
//!   slots). Tags have no data pages.
//! - Values are at most [`MAX_SPARSE_VALUE_SIZE`] bytes. **[OPEN]** overflow pages.
//! - Nothing ever moves, unlike Bevy's swap-remove sparse set, so there are no physical row
//!   positions to fix up on disk. Presence bits are data, not hints, so every change is WAL-logged.
//! - Generations are not checked here; the World checks liveness first.

pub mod presence;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::alloc::Layout;

pub use presence::Presence;
use thiserror::Error;

use crate::Entity;
use crate::page::{HEADER_SIZE, PAGE_SIZE};

pub const MAX_SPARSE_VALUE_SIZE: usize = PAGE_SIZE - HEADER_SIZE;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SparseStoreError {
    #[error("sparse value of {size} bytes exceeds the {max}-byte page capacity")]
    ValueTooLarge { size: usize, max: usize },
}

/// Presence and values of one `SPARSE` component for every entity.
pub struct SparseStore {
    layout: Layout,
    presence: Presence,
    /// `None` for tags.
    data: Option<DataPages>,
}

struct DataPages {
    slots_per_page: usize,
    pages: Vec<Option<DataPage>>,
}

struct DataPage {
    /// Present slots; the page is freed at 0.
    live: u16,
    /// The whole page, header included.
    bytes: Box<PageBuf>,
}

#[repr(C, align(8))]
struct PageBuf([u8; PAGE_SIZE]);

impl SparseStore {
    /// Fails if a value exceeds [`MAX_SPARSE_VALUE_SIZE`].
    pub fn new(layout: Layout) -> Result<Self, SparseStoreError> {
        todo!()
    }

    pub fn len(&self) -> usize {
        self.presence.len()
    }

    pub fn is_empty(&self) -> bool {
        self.presence.is_empty()
    }

    pub fn presence(&self) -> &Presence {
        &self.presence
    }

    pub fn allocated_pages(&self) -> usize {
        self.data.as_ref().map_or(0, |data| {
            data.pages.iter().filter(|page| page.is_some()).count()
        })
    }

    pub fn contains(&self, entity: Entity) -> bool {
        todo!()
    }

    pub fn get(&self, entity: Entity) -> Option<&[u8]> {
        todo!()
    }

    pub fn get_mut(&mut self, entity: Entity) -> Option<&mut [u8]> {
        todo!()
    }

    /// Adds or replaces the value. Returns `true` if it was added.
    pub fn insert(&mut self, entity: Entity, value: &[u8]) -> bool {
        todo!()
    }

    /// Returns `true` if the entity had the component.
    pub fn remove(&mut self, entity: Entity) -> bool {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(index: u32) -> Entity {
        Entity::from_bits((1 << 32) | u64::from(index)).expect("generation is 1")
    }

    fn store(size: usize, align: usize) -> SparseStore {
        let layout = Layout::from_size_align(size, align).expect("valid layout");
        SparseStore::new(layout).expect("value fits a page")
    }

    /* AI GENERATED TEST */
    #[test]
    fn tag_store_tracks_presence_without_data_pages() {
        let mut tags = store(0, 1);

        assert!(tags.insert(entity(7), &[]));

        assert!(tags.contains(entity(7)));
        assert_eq!(tags.get(entity(7)), Some(&[][..]));
        assert_eq!(tags.get(entity(8)), None);
        assert_eq!(tags.allocated_pages(), 0);
        assert!(tags.remove(entity(7)));
        assert!(tags.is_empty());
    }

    /* AI GENERATED TEST */
    #[test]
    fn insert_then_get_returns_the_value() {
        let mut values = store(16, 8);

        assert!(values.insert(entity(3), &[0xAB; 16]));

        assert!(values.contains(entity(3)));
        assert_eq!(values.get(entity(3)), Some(&[0xAB; 16][..]));
        assert_eq!(values.len(), 1);
    }

    /* AI GENERATED TEST */
    #[test]
    fn insert_on_a_present_entity_replaces_the_value() {
        let mut values = store(4, 4);
        values.insert(entity(3), &[1; 4]);

        assert!(!values.insert(entity(3), &[2; 4]));

        assert_eq!(values.get(entity(3)), Some(&[2; 4][..]));
        assert_eq!(values.len(), 1);
    }

    /* AI GENERATED TEST */
    #[test]
    fn absent_entities_have_no_value() {
        let mut values = store(8, 8);
        values.insert(entity(10), &[1; 8]);

        assert_eq!(values.get(entity(11)), None);
        assert_eq!(values.get(entity(5_000_000)), None);
        assert!(!values.contains(entity(5_000_000)));
    }

    /* AI GENERATED TEST */
    #[test]
    fn get_mut_writes_are_visible() {
        let mut values = store(4, 4);
        values.insert(entity(1), &[0; 4]);

        values
            .get_mut(entity(1))
            .expect("present")
            .copy_from_slice(&[9, 8, 7, 6]);

        assert_eq!(values.get(entity(1)), Some(&[9, 8, 7, 6][..]));
        assert_eq!(values.get_mut(entity(2)), None);
    }

    /* AI GENERATED TEST */
    #[test]
    fn entities_in_one_page_range_share_a_page() {
        // 32-byte values: S = 8168 / 32 = 255, so indices 0..=254 share page 0.
        let mut values = store(32, 8);

        values.insert(entity(0), &[1; 32]);
        values.insert(entity(254), &[2; 32]);
        assert_eq!(values.allocated_pages(), 1);

        values.insert(entity(255), &[3; 32]);
        assert_eq!(values.allocated_pages(), 2);

        assert_eq!(values.get(entity(0)), Some(&[1; 32][..]));
        assert_eq!(values.get(entity(254)), Some(&[2; 32][..]));
        assert_eq!(values.get(entity(255)), Some(&[3; 32][..]));
    }

    /* AI GENERATED TEST */
    #[test]
    fn far_apart_entities_allocate_only_their_own_pages() {
        let mut values = store(64, 8);

        values.insert(entity(0), &[1; 64]);
        values.insert(entity(1_000_000), &[2; 64]);

        assert_eq!(values.allocated_pages(), 2);
        assert_eq!(values.get(entity(1_000_000)), Some(&[2; 64][..]));
    }

    /* AI GENERATED TEST */
    #[test]
    fn removing_the_last_value_of_a_page_frees_it() {
        let mut values = store(8, 8);
        values.insert(entity(0), &[1; 8]);
        values.insert(entity(1), &[2; 8]);

        assert!(values.remove(entity(0)));
        assert_eq!(values.allocated_pages(), 1);
        assert_eq!(values.get(entity(1)), Some(&[2; 8][..]));

        assert!(values.remove(entity(1)));
        assert_eq!(values.allocated_pages(), 0);
        assert_eq!(values.get(entity(1)), None);
        assert!(values.is_empty());
    }

    /* AI GENERATED TEST */
    #[test]
    fn removing_an_absent_entity_changes_nothing() {
        let mut values = store(8, 8);
        values.insert(entity(0), &[1; 8]);

        assert!(!values.remove(entity(1)));
        assert!(!values.remove(entity(9_000_000)));

        assert_eq!(values.len(), 1);
        assert_eq!(values.allocated_pages(), 1);
    }

    /* AI GENERATED TEST */
    #[test]
    fn insert_after_a_page_was_freed_reallocates_it() {
        let mut values = store(8, 8);
        values.insert(entity(4), &[1; 8]);
        values.remove(entity(4));

        assert!(values.insert(entity(5), &[2; 8]));

        assert_eq!(values.allocated_pages(), 1);
        assert_eq!(values.get(entity(5)), Some(&[2; 8][..]));
        assert_eq!(values.get(entity(4)), None);
    }

    /* AI GENERATED TEST */
    #[test]
    fn values_are_aligned_to_the_component_alignment() {
        let mut values = store(24, 8);
        for index in 0..10 {
            values.insert(entity(index), &[0; 24]);
        }

        for index in 0..10 {
            let value = values.get(entity(index)).expect("present");
            assert_eq!(value.as_ptr() as usize % 8, 0, "slot {index} misaligned");
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn largest_value_gets_one_page_per_entity() {
        let mut values = store(MAX_SPARSE_VALUE_SIZE, 8);

        values.insert(entity(0), &[1; MAX_SPARSE_VALUE_SIZE]);
        values.insert(entity(1), &[2; MAX_SPARSE_VALUE_SIZE]);

        assert_eq!(values.allocated_pages(), 2);
        assert_eq!(values.get(entity(1)), Some(&[2; MAX_SPARSE_VALUE_SIZE][..]));
    }

    /* AI GENERATED TEST */
    #[test]
    fn value_larger_than_a_page_is_rejected() {
        let layout = Layout::from_size_align(MAX_SPARSE_VALUE_SIZE + 1, 1).expect("valid layout");

        assert_eq!(
            SparseStore::new(layout).err(),
            Some(SparseStoreError::ValueTooLarge {
                size: MAX_SPARSE_VALUE_SIZE + 1,
                max: MAX_SPARSE_VALUE_SIZE,
            })
        );
    }

    /* AI GENERATED TEST */
    #[test]
    #[should_panic(expected = "value size mismatch")]
    fn insert_with_the_wrong_size_panics() {
        let mut values = store(8, 8);

        values.insert(entity(0), &[0; 4]);
    }
}
