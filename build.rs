use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

struct ExampleSpec {
    feature_env: &'static str,
    build_name: &'static str,
    name: &'static str,
    built_path: &'static str,
}

const EXAMPLES: &[ExampleSpec] = &[
    ExampleSpec {
        feature_env: "CARGO_FEATURE_HELLO_WORLD",
        build_name: "hello_world",
        name: "hello_world.elf",
        built_path: "target/hecate-rv32-build/examples-build/hello_world/hello_world.elf",
    },
    ExampleSpec {
        feature_env: "CARGO_FEATURE_LINKED_LIST",
        build_name: "linked_list",
        name: "linked_list.elf",
        built_path: "target/hecate-rv32-build/examples-build/linked_list/linked_list.elf",
    },
    ExampleSpec {
        build_name: "rust_hello",
        feature_env: "CARGO_FEATURE_RUST_HELLO",
        name: "rust_hello.elf",
        built_path: "target/hecate-rv32-build/examples-build/rust_hello/rust_hello.elf",
    },
    ExampleSpec {
        build_name: "vector_contiguous",
        feature_env: "CARGO_FEATURE_VECTOR_CONTIGUOUS",
        name: "vector_contiguous.elf",
        built_path: "target/hecate-rv32-build/examples-build/vector_contiguous/vector_contiguous.elf",
    },
];

fn main() {
    println!("cargo:rerun-if-changed=scripts/build_samples.sh");
    println!("cargo:rerun-if-changed=examples");
    println!("cargo:rerun-if-changed=runtime");

    let target = env::var("TARGET").unwrap_or_default();
    let wasm_target = target.contains("wasm32");

    for example in EXAMPLES {
        println!("cargo:rerun-if-env-changed={}", example.feature_env);
    }

    let enabled: Vec<&ExampleSpec> = if wasm_target {
        Vec::new()
    } else {
        EXAMPLES
            .iter()
            .filter(|example| env::var_os(example.feature_env).is_some())
            .collect()
    };

    if !enabled.is_empty() {
        let selected = enabled
            .iter()
            .map(|example| example.build_name)
            .collect::<Vec<_>>()
            .join(",");

        let status = Command::new("bash")
            .arg("scripts/build_samples.sh")
            .env("HECATE_EXAMPLES", selected)
            .env_remove("RUSTC")
            .env_remove("RUSTDOC")
            .env_remove("RUSTUP_TOOLCHAIN")
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .status()
            .expect("failed to run scripts/build_samples.sh");

        if !status.success() {
            panic!("scripts/build_samples.sh failed");
        }
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("missing OUT_DIR"));
    let generated = out_dir.join("bundled_examples.rs");
    let mut file = fs::File::create(&generated).expect("failed to create bundled_examples.rs");

    writeln!(
        file,
        "pub struct BundledExample {{ pub name: &'static str, pub bytes: &'static [u8], }}"
    )
    .expect("failed to write bundled_examples.rs");
    writeln!(file, "pub const EXAMPLES: &[BundledExample] = &[")
        .expect("failed to write bundled_examples.rs");

    for example in enabled {
        let built_path =
            PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("missing manifest dir"))
                .join(example.built_path);
        if !built_path.exists() {
            panic!("missing built example: {}", built_path.display());
        }

        writeln!(
            file,
            "    BundledExample {{ name: {:?}, bytes: include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{}\")) }},",
            example.name,
            example.built_path,
        )
        .expect("failed to write bundled example entry");
    }

    writeln!(file, "];\n").expect("failed to write bundled_examples.rs");
}
