#[cfg(not(unix))]
compile_error!("ecsql-os supports only Unix for now");

pub mod file;

pub use file::Vfd;
