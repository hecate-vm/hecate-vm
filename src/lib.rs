pub mod debug_ui;
pub mod vm;

pub mod bundled_examples {
    include!(concat!(env!("OUT_DIR"), "/bundled_examples.rs"));
}
