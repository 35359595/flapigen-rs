use std::{env, path::Path};

use flapigen::{CppConfig, CppOptional, CppStrView, CppVariant, LanguageConfig};

fn main() {
    env_logger::init();

    let out_dir = env::var("OUT_DIR").unwrap();

    let cpp_gen_path = Path::new("c++").join("rust_interface");
    let cpp_cfg = if cfg!(feature = "boost") {
        CppConfig::new(cpp_gen_path, "rust".into()).use_boost()
    } else {
        let mut cfg = CppConfig::new(cpp_gen_path, "rust".into())
            .cpp_optional(CppOptional::Boost)
            .cpp_variant(CppVariant::Boost)
            .cpp_str_view(CppStrView::Boost);
        if cfg!(feature = "cpp17_optional") {
            cfg = cfg.cpp_optional(CppOptional::Std17);
        }
        if cfg!(feature = "cpp17_variant") {
            cfg = cfg.cpp_variant(CppVariant::Std17);
        }
        if cfg!(feature = "cpp17_string_view") {
            cfg = cfg.cpp_str_view(CppStrView::Std17);
        }
        cfg
    };

    let swig_gen = flapigen::Generator::new(LanguageConfig::CppConfig(cpp_cfg))
        .rustfmt_bindings(true)
        .remove_not_generated_files_from_output_directory(true);
    let src = Path::new("src").join("cpp_glue.rs.in");
    swig_gen.expand(
        "flapigen_test_c++",
        &src,
        &Path::new(&out_dir).join("cpp_glue.rs"),
    );
    println!("cargo:rerun-if-changed={}", src.display());
}
