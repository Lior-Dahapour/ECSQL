//! In-memory ECS storage core for ECSQL.
//!
//! `no_std` + `alloc`: pure data structures with no OS dependency. Anything that needs files,
//! threads or time belongs in `ecsql-storage`.

#![no_std]

extern crate alloc;

pub mod component;
pub mod entity;
pub mod page;
pub mod table;

pub use component::{
    ComponentError, ComponentId, ComponentInfo, Components, Field, FieldType, StorageKind,
};
pub use entity::{Entities, Entity, EntityError};
