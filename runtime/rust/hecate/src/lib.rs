#![no_std]

pub use hecate_macros::main;
pub use hecate_runtime::*;

pub mod prelude {
    pub use hecate_runtime::prelude::*;
}
