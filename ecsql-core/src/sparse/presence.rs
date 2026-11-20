//! Presence bitmap: one bit per entity index, set when the entity has the component.

use alloc::vec::Vec;

use crate::page::{HEADER_SIZE, PAGE_SIZE};

pub const PRESENCE_BITS_PER_PAGE: usize = (PAGE_SIZE - HEADER_SIZE) * 8;

pub const PRESENCE_WORDS_PER_PAGE: usize = PRESENCE_BITS_PER_PAGE / u64::BITS as usize;

const _: () = assert!(PRESENCE_BITS_PER_PAGE.is_multiple_of(u64::BITS as usize));

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Presence {
    words: Vec<u64>,
    len: usize,
}

impl Presence {
    pub const fn new() -> Self {
        Self {
            words: Vec::new(),
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// return if this bit is on , if this index is out of bound return false
    pub fn contains(&self, index: u32) -> bool {
        let word = index / 64;
        if (word as usize) >= self.words.len() {
            // if it not in len , then it off
            return false;
        }
        self.words[word as usize] & (1 << (index % 64)) != 0
    }

    /// return whatever is bit was newly added ( was clear)
    pub fn insert(&mut self, index: u32) -> bool {
        let word = index / 64;
        if word as usize >= self.words.len() {
            self.words.resize(word as usize + 1, 0);
        }
        let on = self.words[word as usize] & (1 << (index % 64)) != 0;
        if !on {
            self.len += 1;
        }
        self.words[word as usize] |= 1 << (index % 64);
        !on
    }

    /// return true if the bit was set
    pub fn remove(&mut self, index: u32) -> bool {
        let word = index / 64;
        if (word as usize) >= self.words.len() {
            // if it not in len , then it off
            return false;
        }
        let on = self.words[word as usize] & (1 << (index % 64)) != 0;
        if on {
            self.len -= 1;
        }
        self.words[word as usize] &= !(1 << (index % 64));
        on
    }

    pub fn iter(&self) -> PresenceIter<'_> {
        PresenceIter {
            words: &self.words,
            word_index: 0,
            current: self.words.first().copied().unwrap_or(0),
        }
    }
}

pub struct PresenceIter<'a> {
    words: &'a [u64],
    word_index: usize,
    current: u64,
}

impl Iterator for PresenceIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        loop {
            if self.current != 0 {
                let bit = self.current.trailing_zeros();
                // x - 1 turns the lowest ON bit off and the 0s below it on, so the AND clears only that bit.
                self.current &= self.current - 1;
                return Some((self.word_index * 64) as u32 + bit);
            }
            self.word_index += 1;
            if self.word_index >= self.words.len() {
                return None;
            }
            self.current = self.words[self.word_index];
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    /* AI GENERATED TEST */
    #[test]
    fn presence_page_holds_65_344_bits_in_whole_words() {
        assert_eq!(PRESENCE_BITS_PER_PAGE, 65_344);
        assert_eq!(PRESENCE_WORDS_PER_PAGE, 1_021);
    }

    /* AI GENERATED TEST */
    #[test]
    fn new_presence_is_empty() {
        let presence = Presence::new();

        assert!(presence.is_empty());
        assert!(!presence.contains(0));
        assert!(!presence.contains(u32::MAX));
        assert_eq!(presence.iter().next(), None);
    }

    /* AI GENERATED TEST */
    #[test]
    fn insert_reports_whether_the_bit_was_clear() {
        let mut presence = Presence::new();

        assert!(presence.insert(5));
        assert!(!presence.insert(5));

        assert!(presence.contains(5));
        assert_eq!(presence.len(), 1);
    }

    /* AI GENERATED TEST */
    #[test]
    fn remove_reports_whether_the_bit_was_set() {
        let mut presence = Presence::new();
        presence.insert(5);

        assert!(presence.remove(5));
        assert!(!presence.remove(5));
        assert!(!presence.remove(1_000_000));

        assert!(!presence.contains(5));
        assert!(presence.is_empty());
    }

    /* AI GENERATED TEST */
    #[test]
    fn a_single_bit_far_from_zero_is_found() {
        let mut presence = Presence::new();

        presence.insert(1_000_000);

        assert!(presence.contains(1_000_000));
        assert!(!presence.contains(999_999));
    }

    /* AI GENERATED TEST */
    #[test]
    fn indices_past_the_last_word_are_absent_even_with_many_bits_set() {
        let mut presence = Presence::new();
        for index in 0..64 {
            presence.insert(index);
        }

        assert!(!presence.contains(64 * 5));
        assert!(!presence.remove(64 * 5));
        assert_eq!(presence.len(), 64);
    }

    /* AI GENERATED TEST */
    #[test]
    fn bit_i_is_bit_i_mod_64_of_word_i_div_64() {
        // The on-disk format: least significant bit first, so with little-endian words bit i is
        // also bit i % 8 of byte i / 8.
        let mut presence = Presence::new();

        presence.insert(0);
        presence.insert(65);

        assert_eq!(presence.words, [0b1, 0b10]);
    }

    /* AI GENERATED TEST */
    #[test]
    fn removing_a_bit_keeps_its_neighbours() {
        let mut presence = Presence::new();
        for index in [0, 1, 2] {
            presence.insert(index);
        }

        presence.remove(1);

        assert!(presence.contains(0));
        assert!(!presence.contains(1));
        assert!(presence.contains(2));
    }

    /* AI GENERATED TEST */
    #[test]
    fn bits_at_word_boundaries_are_independent() {
        let mut presence = Presence::new();
        for index in [63, 64, 127, 128] {
            presence.insert(index);
        }

        presence.remove(64);

        assert!(presence.contains(63));
        assert!(!presence.contains(64));
        assert!(presence.contains(127));
        assert!(presence.contains(128));
        for index in [0, 62, 65, 126, 129] {
            assert!(!presence.contains(index), "bit {index} should be clear");
        }
        assert_eq!(presence.len(), 3);
    }

    /* AI GENERATED TEST */
    #[test]
    fn iter_yields_set_indices_in_ascending_order() {
        let mut presence = Presence::new();
        let inserted = [1_000_000, 0, 64, 63, 130_000, PRESENCE_BITS_PER_PAGE as u32];
        for index in inserted {
            presence.insert(index);
        }

        let indices: Vec<u32> = presence.iter().collect();

        assert_eq!(
            indices,
            [0, 63, 64, PRESENCE_BITS_PER_PAGE as u32, 130_000, 1_000_000]
        );
    }

    /* AI GENERATED TEST */
    #[test]
    fn iter_returns_every_bit_of_a_full_word() {
        let mut presence = Presence::new();
        for index in 64..128 {
            presence.insert(index);
        }

        let indices: Vec<u32> = presence.iter().collect();

        assert_eq!(indices, (64..128).collect::<Vec<u32>>());
    }

    /* AI GENERATED TEST */
    #[test]
    fn iter_is_empty_after_every_bit_is_removed() {
        let mut presence = Presence::new();
        for index in [3, 700, 90_000] {
            presence.insert(index);
        }
        for index in [3, 700, 90_000] {
            presence.remove(index);
        }

        assert_eq!(presence.iter().next(), None);
    }
}
