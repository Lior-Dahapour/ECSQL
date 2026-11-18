//! Primitive field types and their sizes and alignments.

use crate::Entity;

/// Primitive type of one component field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldType {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Bool,
    /// An entity ID stored as a field (a relation), packed as in [`crate::Entity::to_bits`].
    Entity,
}

impl FieldType {
    /// Size of one value of this type, in bytes.
    pub const fn size(self) -> usize {
        match self {
            Self::I8 => size_of::<i8>(),
            Self::I16 => size_of::<i16>(),
            Self::I32 => size_of::<i32>(),
            Self::I64 => size_of::<i64>(),
            Self::U8 => size_of::<u8>(),
            Self::U16 => size_of::<u16>(),
            Self::U32 => size_of::<u32>(),
            Self::U64 => size_of::<u64>(),
            Self::F32 => size_of::<f32>(),
            Self::F64 => size_of::<f64>(),
            Self::Bool => size_of::<bool>(),
            Self::Entity => size_of::<Entity>(),
        }
    }

    /// Required alignment of one value of this type, in bytes.
    pub const fn align(self) -> usize {
        match self {
            Self::I8 => align_of::<i8>(),
            Self::I16 => align_of::<i16>(),
            Self::I32 => align_of::<i32>(),
            Self::I64 => align_of::<i64>(),
            Self::U8 => align_of::<u8>(),
            Self::U16 => align_of::<u16>(),
            Self::U32 => align_of::<u32>(),
            Self::U64 => align_of::<u64>(),
            Self::F32 => align_of::<f32>(),
            Self::F64 => align_of::<f64>(),
            Self::Bool => align_of::<bool>(),
            Self::Entity => align_of::<Entity>(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_type_sizes_and_alignments() {
        let expected = [
            (FieldType::I8, 1, 1),
            (FieldType::I16, 2, 2),
            (FieldType::I32, 4, 4),
            (FieldType::I64, 8, 8),
            (FieldType::U8, 1, 1),
            (FieldType::U16, 2, 2),
            (FieldType::U32, 4, 4),
            (FieldType::U64, 8, 8),
            (FieldType::F32, 4, 4),
            (FieldType::F64, 8, 8),
            (FieldType::Bool, 1, 1),
            (FieldType::Entity, 8, 8),
        ];

        for (ty, size, align) in expected {
            assert_eq!((ty.size(), ty.align()), (size, align), "{ty:?}");
        }
    }
}
