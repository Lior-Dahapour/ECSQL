//! Runtime component definitions: the catalog of component schemas.

mod align;
mod field;
mod types;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use hashbrown::HashMap;
use thiserror::Error;

use align::RowLayout;

pub use align::align_up;
pub use field::Field;
pub use types::FieldType;

/// Dense handle for a registered component: `0, 1, 2, ...` in registration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(u32);

impl ComponentId {
    /// Position of this component in the registry.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Where a component's values live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageKind {
    /// Inside archetype tables; adding or removing it moves the entity between tables.
    Table,
    /// In a side table keyed by entity; adding or removing it never moves the entity.
    Sparse,
}

/// Schema of one registered component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentInfo {
    id: ComponentId,
    name: Box<str>,
    fields: Box<[Field]>,
    storage: StorageKind,
    size: usize,
    align: usize,
}

impl ComponentInfo {
    /// This component's ID.
    pub fn id(&self) -> ComponentId {
        self.id
    }

    /// This component's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Fields in declaration order.
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// Storage kind chosen at registration.
    pub fn storage(&self) -> StorageKind {
        self.storage
    }

    /// Bytes per value, including padding: the stride of this component's column. 0 for a tag.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Required alignment of each value and of the column's allocation.
    pub fn align(&self) -> usize {
        self.align
    }

    /// Returns `true` if the component has no fields.
    pub fn is_tag(&self) -> bool {
        self.fields.is_empty()
    }

    /// Looks up a field by name.
    ///
    /// # Returns
    ///
    /// The field's position in [`ComponentInfo::fields`] and the field itself, or `None` if no
    /// field has that name.
    pub fn field(&self, name: &str) -> Option<(usize, &Field)> {
        for (i, field) in self.fields.iter().enumerate() {
            if field.name() == name {
                return Some((i, field));
            }
        }
        None
    }
}

/// Errors from registering components.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ComponentError {
    /// A component with this name is already registered.
    #[error("component `{0}` already exists")]
    DuplicateComponent(String),
    /// The same field name appears twice in one definition.
    #[error("component `{component}` declares field `{field}` more than once")]
    DuplicateField { component: String, field: String },
}

/// Registry of all component schemas, indexed by [`ComponentId`] and by name.
#[derive(Debug, Default)]
pub struct Components {
    /// `infos[id.index()]` is the component with that ID.
    infos: Vec<ComponentInfo>,
    by_name: HashMap<Box<str>, ComponentId>,
}

impl Components {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new component schema.
    ///
    /// # Arguments
    ///
    /// * `name` - Component name, unique in the registry
    /// * `fields` - `(field name, type)` pairs in declaration order; empty for a tag
    /// * `storage` - Where the component's values will live
    ///
    /// # Returns
    ///
    /// The new component's ID, the next unused index.
    ///
    /// # Errors
    ///
    /// Returns [`ComponentError::DuplicateComponent`] if `name` is taken, or
    /// [`ComponentError::DuplicateField`] if a field name repeats. On error the registry is
    /// left unchanged.
    ///
    /// # Panics
    ///
    /// Panics if more than `u32::MAX` components would exist.
    pub fn register(
        &mut self,
        name: &str,
        fields: &[(&str, FieldType)],
        storage: StorageKind,
    ) -> Result<ComponentId, ComponentError> {
        if self.id_of(name).is_some() {
            return Err(ComponentError::DuplicateComponent(name.into()));
        }
        for (i, &(field_name, _)) in fields.iter().enumerate() {
            let already_used = fields[..i].iter().any(|f| f.0 == field_name);
            if already_used {
                return Err(ComponentError::DuplicateField {
                    component: name.into(),
                    field: field_name.into(),
                });
            }
        }

        // Phase 2: build.
        let id =
            ComponentId(u32::try_from(self.infos.len()).expect("component id space exhausted"));
        let types: Vec<FieldType> = fields.iter().map(|&(_, ty)| ty).collect();
        let row_layout = RowLayout::compute(&types);
        let fields: Box<[Field]> = fields
            .iter()
            .zip(&row_layout.offsets)
            .map(|(&(field_name, ty), &offset)| Field::new(field_name, ty, offset))
            .collect();
        let info = ComponentInfo {
            id,
            name: name.into(),
            fields,
            storage,
            size: row_layout.size,
            align: row_layout.align,
        };
        self.infos.push(info);
        self.by_name.insert(name.into(), id);
        Ok(id)
    }

    /// Returns the schema for `id`, or `None` if no such component exists.
    pub fn get(&self, id: ComponentId) -> Option<&ComponentInfo> {
        self.infos.get(id.index())
    }

    /// Returns the ID registered under `name`, or `None` if there is none.
    pub fn id_of(&self, name: &str) -> Option<ComponentId> {
        self.by_name.get(name).copied()
    }

    /// Number of registered components.
    pub fn len(&self) -> usize {
        self.infos.len()
    }

    /// Returns `true` if no component is registered.
    pub fn is_empty(&self) -> bool {
        self.infos.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use alloc::borrow::ToOwned;

    use super::*;

    #[test]
    fn register_assigns_sequential_ids() {
        let mut components = Components::new();

        let position = components
            .register(
                "Position",
                &[("x", FieldType::F32), ("y", FieldType::F32)],
                StorageKind::Table,
            )
            .expect("valid definition");
        let health = components
            .register("Health", &[("current", FieldType::I32)], StorageKind::Table)
            .expect("valid definition");

        assert_eq!(position.index(), 0);
        assert_eq!(health.index(), 1);
        assert_eq!(components.len(), 2);
    }

    #[test]
    fn lookup_by_id_and_by_name() {
        let mut components = Components::new();
        let id = components
            .register(
                "Health",
                &[("current", FieldType::I32), ("max", FieldType::I32)],
                StorageKind::Table,
            )
            .expect("valid definition");

        let info = components.get(id).expect("id was registered");

        assert_eq!(components.id_of("Health"), Some(id));
        assert_eq!(components.id_of("Mana"), None);
        assert_eq!(info.name(), "Health");
        assert_eq!(info.id(), id);
        assert_eq!(
            info.field("max").map(|(i, f)| (i, f.ty())),
            Some((1, FieldType::I32))
        );
        assert_eq!(info.field("regen"), None);
    }

    #[test]
    fn tag_component_has_no_fields() {
        let mut components = Components::new();

        let dead = components
            .register("Dead", &[], StorageKind::Table)
            .expect("valid tag");
        let stunned = components
            .register("Stunned", &[], StorageKind::Sparse)
            .expect("valid tag");

        assert!(components.get(dead).expect("registered").is_tag());
        assert_eq!(
            components.get(stunned).expect("registered").storage(),
            StorageKind::Sparse
        );
    }

    #[test]
    fn duplicate_component_is_rejected_without_side_effects() {
        let mut components = Components::new();
        components
            .register("Dead", &[], StorageKind::Table)
            .expect("first is valid");

        let result = components.register("Dead", &[("x", FieldType::I8)], StorageKind::Table);

        assert_eq!(
            result,
            Err(ComponentError::DuplicateComponent("Dead".to_owned()))
        );
        assert_eq!(components.len(), 1);
        assert!(components.get(ComponentId(0)).expect("registered").is_tag());
    }

    #[test]
    fn duplicate_field_is_rejected_without_side_effects() {
        let mut components = Components::new();

        let result = components.register(
            "Bad",
            &[
                ("x", FieldType::I32),
                ("y", FieldType::I32),
                ("x", FieldType::F32),
            ],
            StorageKind::Table,
        );

        assert_eq!(
            result,
            Err(ComponentError::DuplicateField {
                component: "Bad".to_owned(),
                field: "x".to_owned()
            })
        );
        assert!(components.is_empty());
        assert_eq!(components.id_of("Bad"), None);
    }

    #[test]
    fn register_stores_layout() {
        let mut components = Components::new();

        let id = components
            .register(
                "Weird",
                &[
                    ("a", FieldType::U8),
                    ("b", FieldType::F64),
                    ("c", FieldType::U16),
                ],
                StorageKind::Table,
            )
            .expect("valid definition");
        let info = components.get(id).expect("registered");

        assert_eq!((info.size(), info.align()), (16, 8));
        let offsets: Vec<usize> = info.fields().iter().map(Field::offset).collect();
        assert_eq!(offsets, alloc::vec![10, 0, 8]);
    }
}
