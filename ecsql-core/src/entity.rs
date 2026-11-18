//! Entity IDs and their allocator.

use core::cmp::Ordering;
use core::hash::{Hash, Hasher};
use core::num::NonZeroU32;

use alloc::vec::Vec;

use thiserror::Error;

/// A global entity ID that behaves as a `u64`.
///
/// `index` selects a slot in [`Entities`]. `generation` counts how many times that slot has been
/// freed, so an ID kept after its entity was despawned no longer matches a reused slot.
///
/// The packed form is `generation << 32 | index` (see [`Entity::to_bits`]). The struct is laid
/// out to be bit-identical to that `u64` on both endiannesses, so equality, ordering and hashing
/// all compile down to single `u64` operations. `generation` is never zero, which gives
/// `Option<Entity>` a niche and keeps it 8 bytes.
#[derive(Clone, Copy)]
#[repr(C, align(8))]
pub struct Entity {
    // Field order follows the byte order of the packed u64: the low 32 bits come first in memory
    // on little-endian targets and last on big-endian ones.
    #[cfg(target_endian = "little")]
    index: u32,
    generation: NonZeroU32,
    #[cfg(target_endian = "big")]
    index: u32,
}

impl Entity {
    /// Slot index of this entity.
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Generation of the slot when this ID was handed out. Never zero.
    pub const fn generation(self) -> NonZeroU32 {
        self.generation
    }

    /// Packs the ID into a `u64`: generation in the high 32 bits, index in the low 32 bits.
    pub const fn to_bits(self) -> u64 {
        ((self.generation.get() as u64) << 32) | self.index as u64
    }

    /// Unpacks an ID produced by [`Entity::to_bits`].
    ///
    /// # Returns
    ///
    /// `None` if the generation half of `bits` is zero, which no valid ID has.
    pub const fn from_bits(bits: u64) -> Option<Self> {
        match NonZeroU32::new((bits >> 32) as u32) {
            Some(generation) => Some(Self {
                index: bits as u32,
                generation,
            }),
            None => None,
        }
    }
}

impl PartialEq for Entity {
    fn eq(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
}

impl Eq for Entity {}

impl PartialOrd for Entity {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entity {
    /// Orders by the packed `u64`, so generation first, then index.
    fn cmp(&self, other: &Self) -> Ordering {
        self.to_bits().cmp(&other.to_bits())
    }
}

impl Hash for Entity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // One u64 write instead of two u32 writes, so hashers can treat it as an integer key.
        state.write_u64(self.to_bits());
    }
}

impl core::fmt::Debug for Entity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

/// Errors from entity lifecycle operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EntityError {
    /// The ID was never allocated, or its entity was already despawned.
    #[error("entity {0:?} is not alive")]
    NotAlive(Entity),
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    generation: NonZeroU32,
    alive: bool,
}

/// Allocates and frees entity IDs, reusing freed slots.
#[derive(Debug, Default)]
pub struct Entities {
    slots: Vec<Slot>,
    /// Indices of dead slots, reused LIFO so recently freed (cache-warm) slots go first.
    free: Vec<u32>,
}

impl Entities {
    /// Creates an empty allocator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates a new entity ID, reusing a freed slot when one exists.
    ///
    /// # Panics
    ///
    /// Panics if more than `u32::MAX` slots would exist.
    pub fn spawn(&mut self) -> Entity {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.alive = true;
            return Entity {
                index,
                generation: slot.generation,
            };
        }
        let index = u32::try_from(self.slots.len()).expect("entity index space exhausted");
        self.slots.push(Slot {
            generation: NonZeroU32::MIN,
            alive: true,
        });
        Entity {
            index,
            generation: NonZeroU32::MIN,
        }
    }

    /// Frees `entity`'s slot and bumps its generation so the old ID goes stale.
    ///
    /// The generation wraps from `u32::MAX` back to 1 (skipping 0); a stale ID that old could
    /// match again, which is accepted as practically unreachable.
    ///
    /// # Errors
    ///
    /// Returns [`EntityError::NotAlive`] if `entity` is stale or was never allocated.
    pub fn despawn(&mut self, entity: Entity) -> Result<(), EntityError> {
        let slot = self
            .live_slot_mut(entity)
            .ok_or(EntityError::NotAlive(entity))?;
        slot.alive = false;
        slot.generation = slot.generation.checked_add(1).unwrap_or(NonZeroU32::MIN);
        self.free.push(entity.index);
        Ok(())
    }

    /// Returns `true` if `entity` refers to a currently live entity.
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.slots
            .get(entity.index as usize)
            .is_some_and(|slot| slot.alive && slot.generation == entity.generation)
    }

    /// Number of live entities.
    pub fn len(&self) -> usize {
        self.slots.len() - self.free.len()
    }

    /// Returns `true` if no entity is alive.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn live_slot_mut(&mut self, entity: Entity) -> Option<&mut Slot> {
        self.slots
            .get_mut(entity.index as usize)
            .filter(|slot| slot.alive && slot.generation == entity.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(index: u32, generation: u32) -> Entity {
        Entity::from_bits(((generation as u64) << 32) | index as u64).expect("generation != 0")
    }

    #[test]
    fn entity_is_u64_sized_and_niche_optimized() {
        assert_eq!(size_of::<Entity>(), size_of::<u64>());
        assert_eq!(align_of::<Entity>(), align_of::<u64>());
        assert_eq!(size_of::<Option<Entity>>(), size_of::<u64>());
    }

    #[test]
    fn memory_layout_matches_to_bits() {
        let e = entity(0xDEAD_BEEF, 0x1234_5678);

        // SAFETY: Entity is repr(C, align(8)), 8 bytes, with no padding, and every bit pattern
        // with a non-zero generation is a valid u64.
        let raw = unsafe { core::mem::transmute::<Entity, u64>(e) };

        assert_eq!(raw, e.to_bits());
        assert_eq!(raw, 0x1234_5678_DEAD_BEEF);
    }

    #[test]
    fn bits_round_trip_and_reject_zero_generation() {
        let e = entity(7, 3);

        assert_eq!(Entity::from_bits(e.to_bits()), Some(e));
        assert_eq!(Entity::from_bits(7), None);
    }

    #[test]
    fn ordering_matches_packed_u64() {
        let low_gen_high_index = entity(u32::MAX, 1);
        let high_gen_low_index = entity(0, 2);

        assert!(low_gen_high_index < high_gen_low_index);
    }

    #[test]
    fn spawn_allocates_sequential_indices() {
        let mut entities = Entities::new();

        let a = entities.spawn();
        let b = entities.spawn();

        assert_eq!(a, entity(0, 1));
        assert_eq!(b, entity(1, 1));
        assert_eq!(entities.len(), 2);
    }

    #[test]
    fn despawn_reuses_slot_with_new_generation() {
        let mut entities = Entities::new();
        let old = entities.spawn();

        entities.despawn(old).expect("old is alive");
        let new = entities.spawn();

        assert_eq!(new, entity(old.index(), old.generation().get() + 1));
        assert!(!entities.is_alive(old));
        assert!(entities.is_alive(new));
    }

    #[test]
    fn generation_wraps_past_zero() {
        let mut entities = Entities::new();
        let e = entities.spawn();
        entities.slots[0].generation = NonZeroU32::MAX;
        let at_max = entity(e.index(), u32::MAX);

        entities.despawn(at_max).expect("at_max is alive");

        assert_eq!(entities.spawn(), entity(e.index(), 1));
    }

    #[test]
    fn despawn_rejects_stale_and_unknown_ids() {
        let mut entities = Entities::new();
        let e = entities.spawn();
        entities.despawn(e).expect("e is alive");

        let never_allocated = entity(99, 1);

        assert_eq!(entities.despawn(e), Err(EntityError::NotAlive(e)));
        assert_eq!(
            entities.despawn(never_allocated),
            Err(EntityError::NotAlive(never_allocated))
        );
        assert!(entities.is_empty());
    }
}
