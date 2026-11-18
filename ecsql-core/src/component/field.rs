//! A single named, typed field of a component.

use alloc::boxed::Box;

use super::FieldType;

/// One named, typed field of a component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    name: Box<str>,
    ty: FieldType,
    offset: usize,
}

impl Field {
    /// Creates a field. Only the registry builds fields, after computing the row layout.
    ///
    /// # Arguments
    ///
    /// * `name` - Field name, unique within its component
    /// * `ty` - Field type
    /// * `offset` - Byte offset inside one row of the component's column
    pub(super) fn new(name: &str, ty: FieldType, offset: usize) -> Self {
        Self {
            name: name.into(),
            ty,
            offset,
        }
    }

    /// Field name, unique within its component.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Field type.
    pub fn ty(&self) -> FieldType {
        self.ty
    }

    /// Byte offset of this field inside one row of the component's column.
    pub fn offset(&self) -> usize {
        self.offset
    }
}
