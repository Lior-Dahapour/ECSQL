//! Archetype tables: one column per component plus an entity column, sharing row indices.

pub mod column;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::alloc::Layout;

pub use column::Column;

use crate::{ComponentId, Entity};

/// Every entity of one archetype: an entity column plus one column per component.
pub struct Table {
    entities: Vec<Entity>,
    component_ids: Box<[ComponentId]>,
    columns: Box<[Column]>,
}

impl Table {
    /// Creates an empty table with one column per component.
    ///
    /// # Arguments
    ///
    /// * `components` - Component IDs with their row layouts, sorted by ID
    ///
    /// # Panics
    ///
    /// Panics with "component ids must be sorted and unique" if the IDs are out of order or
    /// repeated.
    pub fn new(components: &[(ComponentId, Layout)]) -> Self {
        assert!(
            components.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "component ids must be sorted and unique"
        );
        let component_ids = components.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let mut columns = Vec::with_capacity(components.len());
        for (_, layout) in components {
            columns.push(Column::new(*layout))
        }
        Self {
            entities: Vec::default(),
            component_ids: component_ids.into_boxed_slice(),
            columns: columns.into_boxed_slice(),
        }
    }

    /// Appends `entity` with one row per component, in `component_ids` order.
    ///
    /// # Panics
    ///
    /// Panics with "row count mismatch" or "row size mismatch" before changing anything.
    pub fn push(&mut self, entity: Entity, rows: &[&[u8]]) {
        assert_eq!(rows.len(), self.columns.len(), "row count mismatch");
        assert!(
            self.columns
                .iter()
                .zip(rows)
                .all(|(column, row)| row.len() == column.item_layout().size()),
            "row size mismatch"
        );
        // first reserve one row
        self.reserve(1);
        self.entities.push(entity);
        for (i, row) in rows.iter().enumerate() {
            self.columns[i].push(row);
        }
    }

    /// Ensures every column and the entity list have room for `additional` more rows.
    ///
    /// # Panics
    ///
    /// Panics with "capacity overflow" if a buffer size would not fit in `isize::MAX` bytes.
    pub fn reserve(&mut self, additional: usize) {
        for cols in self.columns.iter_mut() {
            cols.reserve(additional);
        }
        self.entities.reserve(additional);
    }

    /// Removes `row` from every column in O(1) by moving the last row into its place.
    ///
    /// # Returns
    ///
    /// The entity that now lives at `row`, whose location the caller must update, or `None` if
    /// `row` was the last row.
    ///
    /// # Panics
    ///
    /// Panics with "swap_remove row out of bounds" if `row >= len`, before changing anything.
    pub fn swap_remove(&mut self, row: usize) -> Option<Entity> {
        assert!(row < self.len(), "swap_remove row out of bounds");
        for column in self.columns.iter_mut() {
            column.swap_remove(row);
        }
        self.entities.swap_remove(row);
        self.entities.get(row).copied()
    }

    /// Moves the entity at `row` into `dst`, the table for its new component set.
    ///
    /// Components in both tables move across, components only in `self` are dropped, and
    /// components only in `dst` are filled from `added`, in `dst`'s ID order.
    ///
    /// # Returns
    ///
    /// `(dst_row, moved)`: the entity's new row in `dst`, and the entity that moved into `row`
    /// here, as from [`Table::swap_remove`].
    ///
    /// `added` must hold exactly one row, of the right size, for each component in `dst` but not
    /// in `self`, in ID order. The caller (the World) guarantees this from the archetype edge.
    ///
    /// # Panics
    ///
    /// Panics with "move_row_to row out of bounds" if `row >= len`, before changing anything.
    pub fn move_row_to(
        &mut self,
        row: usize,
        dst: &mut Table,
        added: &[&[u8]],
    ) -> (usize, Option<Entity>) {
        assert!(row < self.len(), "move_row_to row out of bounds");
        dst.reserve(1);
        let (mut i, mut j, mut k) = (0, 0, 0);
        loop {
            match (self.component_ids.get(i), dst.component_ids.get(j)) {
                (None, None) => break,
                // in both tables: copy the row across
                (Some(src_id), Some(dst_id)) if src_id == dst_id => {
                    self.columns[i].move_row_to(row, &mut dst.columns[j]);
                    i += 1;
                    j += 1;
                }
                // only in dst (dst's id is smaller, or src is finished): take the next added row
                (Some(src_id), Some(dst_id)) if dst_id < src_id => {
                    dst.columns[j].push(added[k]);
                    k += 1;
                    j += 1;
                }
                (None, Some(_)) => {
                    dst.columns[j].push(added[k]);
                    k += 1;
                    j += 1;
                }
                // only in self (src's id is smaller, or dst is finished): drop it
                (Some(_), _) => {
                    self.columns[i].swap_remove(row); // bye bye
                    i += 1
                }
            }
        }
        let dst_row = dst.len();
        dst.entities.push(self.entities[row]);
        self.entities.swap_remove(row);
        (dst_row, self.entities.get(row).copied())
    }

    /// Returns the number of rows (entities) in the table.
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Returns `true` if the table holds no entities.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the entity stored at `row`, or `None` if it is out of bounds.
    pub fn entity(&self, row: usize) -> Option<Entity> {
        self.entities.get(row).cloned()
    }

    /// Returns the table's component IDs in ascending order.
    pub fn component_ids(&self) -> &[ComponentId] {
        self.component_ids.as_ref()
    }

    /// Returns the column storing `id`, or `None` if the table does not have that component.
    pub fn column(&self, id: ComponentId) -> Option<&Column> {
        let comp_idx = self.component_ids.binary_search(&id).ok()?;
        self.columns.get(comp_idx)
    }

    /// Returns the column storing `id` for writing, or `None` if the table lacks it.
    pub fn column_mut(&mut self, id: ComponentId) -> Option<&mut Column> {
        let comp_idx = self.component_ids.binary_search(&id).ok()?;
        self.columns.get_mut(comp_idx)
    }
}

#[cfg(test)]
mod tests {
    // The crate is `no_std`, but the test harness links `std`; we need it for `catch_unwind`.
    extern crate std;

    use alloc::vec::Vec;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;
    use crate::{Components, StorageKind};

    fn entity(index: u32) -> Entity {
        Entity::from_bits((1 << 32) | u64::from(index)).expect("generation is 1")
    }

    /// Table with an 8-byte, a 2-byte and a tag component.
    fn mixed_table() -> (Table, Vec<ComponentId>) {
        let ids = component_ids(3);
        let table = Table::new(&[
            (ids[0], layout(8, 8)),
            (ids[1], layout(2, 2)),
            (ids[2], layout(0, 1)),
        ]);
        (table, ids)
    }

    /// Text of a caught panic: `panic!("literal")` carries a `&str`, formatted panics a `String`.
    fn panic_message(payload: std::boxed::Box<dyn core::any::Any + Send>) -> std::string::String {
        match payload.downcast::<std::string::String>() {
            Ok(message) => *message,
            Err(payload) => std::string::String::from(
                *payload
                    .downcast::<&str>()
                    .expect("panic payload is a string"),
            ),
        }
    }

    fn assert_columns_in_step(table: &Table) {
        for &id in table.component_ids() {
            let column = table.column(id).expect("column exists");
            assert_eq!(column.len(), table.len(), "column {id:?} out of step");
        }
    }

    /// Registers `count` field-less components and returns their IDs, which come out sorted.
    fn component_ids(count: usize) -> Vec<ComponentId> {
        const NAMES: [&str; 5] = ["A", "B", "C", "D", "E"];
        let mut registry = Components::default();
        NAMES[..count]
            .iter()
            .map(|name| {
                registry
                    .register(name, &[], StorageKind::Table)
                    .expect("unique name")
            })
            .collect()
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).expect("valid layout")
    }

    /* AI GENERATED TEST */
    #[test]
    fn new_table_is_empty() {
        let ids = component_ids(2);

        let table = Table::new(&[(ids[0], layout(8, 8)), (ids[1], layout(4, 4))]);

        assert_eq!(table.len(), 0);
        assert!(table.is_empty());
        assert_eq!(table.entity(0), None);
    }

    /* AI GENERATED TEST */
    #[test]
    fn new_creates_one_empty_column_per_component_with_its_layout() {
        let ids = component_ids(3);
        let layouts = [layout(16, 8), layout(0, 1), layout(2, 2)];
        let components: Vec<_> = ids.iter().copied().zip(layouts).collect();

        let table = Table::new(&components);

        for (id, expected) in components {
            let column = table.column(id).expect("column exists");
            assert!(column.is_empty());
            assert_eq!(column.item_layout(), expected);
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn component_ids_are_returned_in_ascending_order() {
        let ids = component_ids(3);

        let table = Table::new(&[(ids[0], layout(4, 4)), (ids[2], layout(4, 4))]);

        assert_eq!(table.component_ids(), &[ids[0], ids[2]]);
    }

    /* AI GENERATED TEST */
    #[test]
    fn column_lookup_for_missing_component_is_none() {
        let ids = component_ids(4);
        let mut table = Table::new(&[(ids[0], layout(4, 4)), (ids[2], layout(4, 4))]);

        // Missing in the middle, past the end, and in a table that has neighbours on both sides.
        assert!(table.column(ids[1]).is_none());
        assert!(table.column(ids[3]).is_none());
        assert!(table.column_mut(ids[1]).is_none());
        assert!(table.column_mut(ids[2]).is_some());
    }

    /* AI GENERATED TEST */
    #[test]
    fn table_without_components_is_valid() {
        // The archetype of an entity with no components at all.
        let table = Table::new(&[]);

        assert!(table.is_empty());
        assert!(table.component_ids().is_empty());
    }

    /* AI GENERATED TEST */
    #[test]
    #[should_panic(expected = "component ids must be sorted and unique")]
    fn new_panics_on_unsorted_ids() {
        let ids = component_ids(2);

        Table::new(&[(ids[1], layout(4, 4)), (ids[0], layout(4, 4))]);
    }

    /* AI GENERATED TEST */
    #[test]
    #[should_panic(expected = "component ids must be sorted and unique")]
    fn new_panics_on_duplicate_ids() {
        let ids = component_ids(1);

        Table::new(&[(ids[0], layout(4, 4)), (ids[0], layout(4, 4))]);
    }

    /* AI GENERATED TEST */
    #[test]
    fn push_adds_the_entity_and_one_row_to_every_column() {
        let (mut table, ids) = mixed_table();

        table.push(entity(7), &[&[1; 8], &[2; 2], &[]]);

        assert_eq!(table.len(), 1);
        assert_eq!(table.entity(0), Some(entity(7)));
        assert_eq!(
            table.column(ids[0]).and_then(|c| c.get(0)),
            Some(&[1; 8][..])
        );
        assert_eq!(
            table.column(ids[1]).and_then(|c| c.get(0)),
            Some(&[2; 2][..])
        );
        assert_eq!(table.column(ids[2]).and_then(|c| c.get(0)), Some(&[][..]));
        assert_columns_in_step(&table);
    }

    /* AI GENERATED TEST */
    #[test]
    fn many_pushes_keep_every_column_in_step() {
        let (mut table, ids) = mixed_table();

        for index in 0..100u8 {
            table.push(entity(u32::from(index)), &[&[index; 8], &[index; 2], &[]]);
        }

        assert_eq!(table.len(), 100);
        assert_columns_in_step(&table);
        assert_eq!(table.entity(42), Some(entity(42)));
        assert_eq!(
            table.column(ids[0]).and_then(|c| c.get(42)),
            Some(&[42; 8][..])
        );
    }

    /* AI GENERATED TEST */
    #[test]
    fn push_into_table_without_components_only_stores_the_entity() {
        let mut table = Table::new(&[]);

        table.push(entity(3), &[]);

        assert_eq!(table.len(), 1);
        assert_eq!(table.entity(0), Some(entity(3)));
    }

    /* AI GENERATED TEST */
    #[test]
    fn push_with_too_few_rows_panics_without_changing_the_table() {
        let (mut table, _) = mixed_table();

        let result = catch_unwind(AssertUnwindSafe(|| table.push(entity(1), &[&[0; 8]])));

        let message = panic_message(result.expect_err("push must panic"));
        assert!(message.contains("row count mismatch"), "{message}");
        assert!(table.is_empty());
        assert_columns_in_step(&table);
    }

    /* AI GENERATED TEST */
    #[test]
    fn push_with_too_many_rows_panics_without_changing_the_table() {
        let (mut table, _) = mixed_table();

        let result = catch_unwind(AssertUnwindSafe(|| {
            table.push(entity(1), &[&[0; 8], &[0; 2], &[], &[0; 4]]);
        }));

        let message = panic_message(result.expect_err("push must panic"));
        assert!(message.contains("row count mismatch"), "{message}");
        assert!(table.is_empty());
        assert_columns_in_step(&table);
    }

    /* AI GENERATED TEST */
    #[test]
    fn push_with_a_bad_late_row_panics_without_touching_earlier_columns() {
        let (mut table, _) = mixed_table();
        table.push(entity(1), &[&[1; 8], &[1; 2], &[]]);

        // The first two rows are fine; the third (a tag) has the wrong size.
        let result = catch_unwind(AssertUnwindSafe(|| {
            table.push(entity(2), &[&[2; 8], &[2; 2], &[2; 1]]);
        }));

        let message = panic_message(result.expect_err("push must panic"));
        assert!(message.contains("row size mismatch"), "{message}");
        assert_eq!(table.len(), 1);
        assert_columns_in_step(&table);
    }

    /// `mixed_table` holding one entity per index; each row's bytes are the index.
    fn filled_table(indices: &[u8]) -> (Table, Vec<ComponentId>) {
        let (mut table, ids) = mixed_table();
        for &index in indices {
            table.push(entity(u32::from(index)), &[&[index; 8], &[index; 2], &[]]);
        }
        (table, ids)
    }

    /// Asserts that `row` holds `index`'s entity and its bytes in every column.
    fn assert_row_is(table: &Table, ids: &[ComponentId], row: usize, index: u8) {
        assert_eq!(
            table.entity(row),
            Some(entity(u32::from(index))),
            "entity at row {row}"
        );
        assert_eq!(
            table.column(ids[0]).and_then(|c| c.get(row)),
            Some(&[index; 8][..])
        );
        assert_eq!(
            table.column(ids[1]).and_then(|c| c.get(row)),
            Some(&[index; 2][..])
        );
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_moves_last_row_into_the_hole_and_returns_its_entity() {
        let (mut table, ids) = filled_table(&[10, 20, 30]);

        let moved = table.swap_remove(0);

        assert_eq!(moved, Some(entity(30)));
        assert_eq!(table.len(), 2);
        assert_row_is(&table, &ids, 0, 30);
        assert_row_is(&table, &ids, 1, 20);
        assert_columns_in_step(&table);
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_last_row_returns_none() {
        let (mut table, ids) = filled_table(&[10, 20, 30]);

        let moved = table.swap_remove(2);

        assert_eq!(moved, None);
        assert_eq!(table.len(), 2);
        assert_row_is(&table, &ids, 0, 10);
        assert_row_is(&table, &ids, 1, 20);
        assert_columns_in_step(&table);
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_only_row_empties_the_table() {
        let (mut table, _) = filled_table(&[10]);

        let moved = table.swap_remove(0);

        assert_eq!(moved, None);
        assert!(table.is_empty());
        assert_columns_in_step(&table);
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_from_the_front_reports_each_moved_entity() {
        let (mut table, ids) = filled_table(&[0, 1, 2, 3, 4]);

        // Each removal at row 0 pulls the current last entity forward.
        for expected in [4, 3, 2, 1] {
            assert_eq!(table.swap_remove(0), Some(entity(expected)));
            assert_row_is(&table, &ids, 0, expected as u8);
            assert_columns_in_step(&table);
        }
        assert_eq!(table.swap_remove(0), None);

        assert!(table.is_empty());
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_in_table_without_components_moves_entities() {
        let mut table = Table::new(&[]);
        table.push(entity(1), &[]);
        table.push(entity(2), &[]);

        let moved = table.swap_remove(0);

        assert_eq!(moved, Some(entity(2)));
        assert_eq!(table.entity(0), Some(entity(2)));
        assert_eq!(table.len(), 1);
    }

    /* AI GENERATED TEST */
    #[test]
    fn swap_remove_out_of_bounds_panics_without_changing_the_table() {
        let (mut table, ids) = filled_table(&[10, 20]);

        let result = catch_unwind(AssertUnwindSafe(|| table.swap_remove(2)));

        let message = panic_message(result.expect_err("swap_remove must panic"));
        assert!(
            message.contains("swap_remove row out of bounds"),
            "{message}"
        );
        assert_eq!(table.len(), 2);
        assert_row_is(&table, &ids, 0, 10);
        assert_row_is(&table, &ids, 1, 20);
        assert_columns_in_step(&table);
    }

    /// Row size of each test component, indexed like `component_ids(4)`: A, B, C, D.
    const SIZES: [usize; 4] = [8, 2, 4, 1];

    /// Table holding components `which` (ascending indices into `ids`).
    fn table_of(ids: &[ComponentId], which: &[usize]) -> Table {
        let components: Vec<_> = which
            .iter()
            .map(|&k| (ids[k], layout(SIZES[k], SIZES[k])))
            .collect();
        Table::new(&components)
    }

    /// Bytes of component `k` for entity `index`: `SIZES[k]` copies of `index`.
    fn bytes(k: usize, index: u8) -> Vec<u8> {
        alloc::vec![index; SIZES[k]]
    }

    fn push_entities(table: &mut Table, which: &[usize], indices: &[u8]) {
        for &index in indices {
            let rows: Vec<Vec<u8>> = which.iter().map(|&k| bytes(k, index)).collect();
            let row_refs: Vec<&[u8]> = rows.iter().map(Vec::as_slice).collect();
            table.push(entity(u32::from(index)), &row_refs);
        }
    }

    /// Asserts that `row` holds entity `index` and its bytes in every component in `which`.
    fn assert_holds(table: &Table, ids: &[ComponentId], which: &[usize], row: usize, index: u8) {
        assert_eq!(
            table.entity(row),
            Some(entity(u32::from(index))),
            "entity at row {row}"
        );
        for &k in which {
            let column = table.column(ids[k]).expect("column exists");
            assert_eq!(
                column.get(row),
                Some(&bytes(k, index)[..]),
                "component {k}, row {row}"
            );
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn move_row_to_adding_a_component_moves_shared_rows_and_fills_the_new_one() {
        let ids = component_ids(4);
        let mut src = table_of(&ids, &[0, 1]);
        let mut dst = table_of(&ids, &[0, 1, 2]);
        push_entities(&mut src, &[0, 1], &[10, 20, 30]);

        let result = src.move_row_to(0, &mut dst, &[&bytes(2, 10)]);

        assert_eq!(result, (0, Some(entity(30))));
        assert_holds(&dst, &ids, &[0, 1, 2], 0, 10);
        assert_eq!(src.len(), 2);
        assert_holds(&src, &ids, &[0, 1], 0, 30);
        assert_holds(&src, &ids, &[0, 1], 1, 20);
        assert_columns_in_step(&src);
        assert_columns_in_step(&dst);
    }

    /* AI GENERATED TEST */
    #[test]
    fn move_row_to_removing_a_component_drops_its_row() {
        let ids = component_ids(4);
        let mut src = table_of(&ids, &[0, 1, 2]);
        let mut dst = table_of(&ids, &[0, 2]);
        push_entities(&mut src, &[0, 1, 2], &[10, 20]);

        let result = src.move_row_to(1, &mut dst, &[]);

        assert_eq!(result, (0, None));
        assert_holds(&dst, &ids, &[0, 2], 0, 20);
        assert_eq!(src.len(), 1);
        assert_holds(&src, &ids, &[0, 1, 2], 0, 10);
        assert_columns_in_step(&src);
        assert_columns_in_step(&dst);
    }

    /* AI GENERATED TEST */
    #[test]
    fn move_row_to_with_components_added_and_removed_on_both_sides() {
        let ids = component_ids(4);
        // {A, C} -> {B, C, D}: A is dropped, C moves, B and D come from `added` in ID order.
        let mut src = table_of(&ids, &[0, 2]);
        let mut dst = table_of(&ids, &[1, 2, 3]);
        push_entities(&mut src, &[0, 2], &[5]);

        let result = src.move_row_to(0, &mut dst, &[&bytes(1, 5), &bytes(3, 5)]);

        assert_eq!(result, (0, None));
        assert_holds(&dst, &ids, &[1, 2, 3], 0, 5);
        assert!(src.is_empty());
        assert_columns_in_step(&src);
        assert_columns_in_step(&dst);
    }

    /* AI GENERATED TEST */
    #[test]
    fn move_row_to_appends_after_existing_destination_rows() {
        let ids = component_ids(4);
        let mut src = table_of(&ids, &[0]);
        let mut dst = table_of(&ids, &[0]);
        push_entities(&mut src, &[0], &[1]);
        push_entities(&mut dst, &[0], &[8, 9]);

        let (dst_row, _) = src.move_row_to(0, &mut dst, &[]);

        assert_eq!(dst_row, 2);
        assert_holds(&dst, &ids, &[0], 0, 8);
        assert_holds(&dst, &ids, &[0], 1, 9);
        assert_holds(&dst, &ids, &[0], 2, 1);
    }

    /* AI GENERATED TEST */
    #[test]
    fn move_row_to_table_without_components_keeps_only_the_entity() {
        let ids = component_ids(4);
        let mut src = table_of(&ids, &[0, 1]);
        let mut dst = Table::new(&[]);
        push_entities(&mut src, &[0, 1], &[3, 4]);

        let result = src.move_row_to(0, &mut dst, &[]);

        assert_eq!(result, (0, Some(entity(4))));
        assert_eq!(dst.entity(0), Some(entity(3)));
        assert_holds(&src, &ids, &[0, 1], 0, 4);
    }

    /// Runs a `move_row_to` expected to panic and checks that neither table changed.
    fn assert_failed_move_changes_nothing(row: usize, added: &[&[u8]], expected: &str) {
        let ids = component_ids(4);
        let mut src = table_of(&ids, &[0]);
        let mut dst = table_of(&ids, &[0, 1]);
        push_entities(&mut src, &[0], &[1, 2]);

        let result = catch_unwind(AssertUnwindSafe(|| src.move_row_to(row, &mut dst, added)));

        let message = panic_message(result.expect_err("move_row_to must panic"));
        assert!(message.contains(expected), "{message}");
        assert_eq!(src.len(), 2);
        assert_holds(&src, &ids, &[0], 0, 1);
        assert_holds(&src, &ids, &[0], 1, 2);
        assert!(dst.is_empty());
        assert_columns_in_step(&src);
        assert_columns_in_step(&dst);
    }

    /* AI GENERATED TEST */
    #[test]
    fn move_row_to_out_of_bounds_panics_without_changing_either_table() {
        assert_failed_move_changes_nothing(2, &[&[0; 2]], "move_row_to row out of bounds");
    }

    /// Capacity of every column and of the entity list.
    fn capacities(table: &Table) -> Vec<usize> {
        let mut capacities: Vec<usize> = table
            .component_ids()
            .iter()
            .map(|&id| table.column(id).expect("column exists").capacity())
            .collect();
        capacities.push(table.entities.capacity());
        capacities
    }

    /* AI GENERATED TEST */
    #[test]
    fn reserve_grows_every_column_and_the_entity_list_without_adding_rows() {
        let (mut table, _) = mixed_table();

        table.reserve(10);

        assert!(table.is_empty());
        assert!(capacities(&table).iter().all(|&capacity| capacity >= 10));
        assert_columns_in_step(&table);
    }

    /* AI GENERATED TEST */
    #[test]
    fn reserve_counts_from_the_current_length() {
        let (mut table, _) = filled_table(&[1, 2, 3]);

        table.reserve(5);

        assert_eq!(table.len(), 3);
        assert!(capacities(&table).iter().all(|&capacity| capacity >= 8));
    }

    /* AI GENERATED TEST */
    #[test]
    fn pushes_within_reserved_capacity_do_not_reallocate() {
        let (mut table, _) = mixed_table();
        table.reserve(10);
        let before = capacities(&table);

        for index in 0..10u8 {
            table.push(entity(u32::from(index)), &[&[index; 8], &[index; 2], &[]]);
        }

        assert_eq!(capacities(&table), before);
    }

    /* AI GENERATED TEST */
    #[test]
    #[should_panic(expected = "capacity overflow")]
    fn reserve_panics_on_capacity_overflow() {
        let (mut table, _) = mixed_table();

        table.reserve(usize::MAX / 4);
    }
}
