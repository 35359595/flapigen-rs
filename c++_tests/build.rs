extern crate env_logger;
extern crate rust_swig;
extern crate syntex;

use std::time::Instant;
use std::env;
use std::path::Path;
use rust_swig::{CppConfig, LanguageConfig};

fn main() {
    env_logger::init().unwrap();

    let now = Instant::now();

    let out_dir = env::var("OUT_DIR").unwrap();
    rust_swig_expand(
        Path::new("src/lib.rs.in"),
        &Path::new(&out_dir).join("lib.rs"),
    ).unwrap();
    let expand_time = now.elapsed();
    println!(
        "rust swig expand time: {}",
        expand_time.as_secs() as f64 + (expand_time.subsec_nanos() as f64) / 1_000_000_000.
    );
    println!("cargo:rerun-if-changed=src");
    //rebuild if user remove generated code
    println!("cargo:rerun-if-changed={}", out_dir);
}

fn rust_swig_expand(from: &Path, out: &Path) -> Result<(), String> {
    println!("Run rust_swig_expand");
    let mut registry = syntex::Registry::new();
    let swig_gen = rust_swig::Generator::new(LanguageConfig::CppConfig(CppConfig::new(
        Path::new("c++").join("rust_interface"),
        "rust".into(),
    )));
    swig_gen.register(&mut registry);
    registry
        .expand("rust_swig_test_c++", from, out)
        .map_err(|err| format!("rust swig macros expand failed: {}", err))
}
