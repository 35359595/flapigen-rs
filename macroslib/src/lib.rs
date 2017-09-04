//! `rust_swig` is a Rust Simplified Wrapper and Interface Generator used
//! to connect other programming languages to Rust.
//! The idea of this softwared based on [swig](http://www.swig.org).
//! For macros expansion it uses [syntex](https://crates.io/crates/syntex).
//! More details can be found at
//! [README](https://github.com/Dushistov/rust_swig/blob/master/README.md)
#[macro_use]
extern crate bitflags;
#[cfg(test)]
extern crate env_logger;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
extern crate petgraph;
extern crate syntex;
extern crate syntex_errors;
extern crate syntex_pos;
extern crate syntex_syntax;

macro_rules! unwrap_presult {
    ($presult_epxr:expr) => {
        match $presult_epxr {
            Ok(x) => x,
            Err(mut err) => {
                err.emit();
                panic!("rust_swig fatal error, see above");
            }
        }
    };
    ($presult_epxr:expr, $conv_map:expr) => {
        match $presult_epxr {
            Ok(x) => x,
            Err(mut err) => {
                debug!("{}", $conv_map);
                err.emit();
                panic!("rust_swig fatal error, see above");
            }
        }
    };
}

mod types_conv_map;
mod java_jni;
mod errors;
mod parsing;
mod my_ast;

use std::path::PathBuf;
use std::cell::RefCell;
use std::rc::Rc;
use std::env;
use std::str::FromStr;

use syntex_syntax::parse::ParseSess;
use syntex_syntax::codemap::Span;
use syntex::Registry;
use syntex_syntax::tokenstream::TokenTree;
use syntex_syntax::ext::base::{ExtCtxt, MacEager, MacResult, TTMacroExpander};
use syntex_syntax::parse::PResult;
use syntex_syntax::ptr::P;
use syntex_syntax::ast;
use syntex_pos::DUMMY_SP;
use syntex_syntax::symbol::Symbol;
use syntex_syntax::util::small_vector::SmallVector;

use types_conv_map::TypesConvMap;
use errors::fatal_error;
use parsing::{parse_foreign_enum, parse_foreign_interface, parse_foreigner_class};

/// Calculate target pointer width from environment variable
/// that `cargo` inserts
pub fn target_pointer_width_from_env() -> usize {
    let p_width = env::var("CARGO_CFG_TARGET_POINTER_WIDTH")
        .expect("No CARGO_CFG_TARGET_POINTER_WIDTH environment variable");
    <usize>::from_str(&p_width).expect("Can not convert CARGO_CFG_TARGET_POINTER_WIDTH to usize")
}

/// `LanguageConfig` contains configuration for specific programming language
pub enum LanguageConfig {
    #[deprecated(since = "0.1.0", note = "please use `JavaConfig` instead")]
    Java {
        /// directory where place generated java files
        output_dir: PathBuf,
        /// package name for generated java files
        package_name: String,
    },
    JavaConfig(JavaConfig),
}

trait LanguageGenerator {
    fn generate<'a>(
        &self,
        sess: &'a ParseSess,
        conv_map: &mut TypesConvMap,
        pointer_target_width: usize,
        class: &ForeignerClassInfo,
    ) -> PResult<'a, Vec<P<ast::Item>>>;

    fn generate_enum<'a>(
        &self,
        sess: &'a ParseSess,
        conv_map: &mut TypesConvMap,
        pointer_target_width: usize,
        enum_info: &ForeignEnumInfo,
    ) -> PResult<'a, Vec<P<ast::Item>>>;

    fn generate_interface<'a>(
        &self,
        sess: &'a ParseSess,
        conv_map: &mut TypesConvMap,
        pointer_target_width: usize,
        interace: &ForeignInterface,
    ) -> PResult<'a, Vec<P<ast::Item>>>;
}

/// `Generator` is a main point of `rust_swig`.
/// It expands rust macroses and generates not rust code.
/// It designed to use inside `build.rs`.
pub struct Generator {
    // Because of API of syntex, to register for several macroses
    data: Rc<RefCell<GeneratorData>>,
}

struct GeneratorData {
    init_done: bool,
    config: LanguageConfig,
    conv_map: TypesConvMap,
    conv_map_source: Vec<TypesConvMapCode>,
    pointer_target_width: usize,
}

struct TypesConvMapCode {
    id_of_code: &'static str,
    code: &'static str,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum SelfTypeVariant {
    RptrMut,
    Rptr,
    Mut,
    Default,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum MethodVariant {
    Constructor,
    Method(SelfTypeVariant),
    StaticMethod,
}

#[derive(Debug, Clone)]
struct ForeignerMethod {
    variant: MethodVariant,
    rust_id: ast::Path,
    fn_decl: P<ast::FnDecl>,
    name_alias: Option<Symbol>,
    /// cache if rust_fn_decl.output == Result
    may_return_error: bool,
    foreigner_private: bool,
    doc_comments: Vec<Symbol>,
}

impl ForeignerMethod {
    fn short_name(&self) -> Symbol {
        if let Some(name) = self.name_alias {
            name
        } else {
            match self.rust_id.segments.len() {
                0 => Symbol::intern(""),
                n => self.rust_id.segments[n - 1].identifier.name,
            }
        }
    }

    fn span(&self) -> Span {
        self.rust_id.span
    }
}

#[derive(Debug, Clone)]
struct ForeignerClassInfo {
    name: Symbol,
    methods: Vec<ForeignerMethod>,
    self_type: ast::Path,
    /// Not necessarily equal to self_type, may be for example Rc<self_type>
    this_type_for_method: Option<ast::Ty>,
    foreigner_code: String,
    /// For example if we have `fn new(x: X) -> Result<Y, Z>`, then Result<Y, Z>
    constructor_ret_type: Option<ast::Ty>,
    span: Span,
    doc_comments: Vec<Symbol>,
}

#[derive(Debug, Clone)]
struct ForeignEnumItem {
    name: Symbol,
    span: Span,
    rust_name: ast::Path,
    doc_comments: Vec<Symbol>,
}

#[derive(Debug, Clone)]
struct ForeignEnumInfo {
    name: Symbol,
    span: Span,
    items: Vec<ForeignEnumItem>,
    doc_comments: Vec<Symbol>,
}

impl ForeignEnumInfo {
    fn rust_enum_name(&self) -> Symbol {
        self.name
    }
}

struct ForeignInterfaceMethod {
    name: Symbol,
    rust_name: ast::Path,
    fn_decl: P<ast::FnDecl>,
    doc_comments: Vec<Symbol>,
}

struct ForeignInterface {
    name: Symbol,
    self_type: ast::Path,
    doc_comments: Vec<Symbol>,
    items: Vec<ForeignInterfaceMethod>,
    span: Span,
}

impl Generator {
    #[deprecated(since = "0.1.0", note = "please use `new_with_pointer_target_width` instead")]
    pub fn new(config: LanguageConfig) -> Generator {
        Generator::new_with_pointer_target_width(config, target_pointer_width_from_env())
    }

    #[allow(deprecated)]
    pub fn new_with_pointer_target_width(
        config: LanguageConfig,
        pointer_target_width: usize,
    ) -> Generator {
        let mut conv_map_source = Vec::new();
        match config {
            LanguageConfig::Java { .. } | LanguageConfig::JavaConfig(..) => {
                conv_map_source.push(TypesConvMapCode {
                    id_of_code: "jni-include.rs",
                    code: include_str!("java_jni/jni-include.rs"),
                });
            }
        }
        Generator {
            data: Rc::new(RefCell::new(GeneratorData {
                init_done: false,
                config,
                conv_map: TypesConvMap::default(),
                conv_map_source,
                pointer_target_width,
            })),
        }
    }

    pub fn register(self, registry: &mut Registry) {
        registry.add_macro("foreign_enum", EnumHandler(self.data.clone()));
        registry.add_macro("foreign_interface", InterfaceHandler(self.data.clone()));
        registry.add_macro("foreigner_class", self);
    }

    /// Add new foreign langauge type <-> Rust mapping
    pub fn merge_type_map(self, id_of_code: &'static str, code: &'static str) -> Generator {
        self.data
            .borrow_mut()
            .conv_map_source
            .push(TypesConvMapCode { id_of_code, code });
        self
    }
}

impl TTMacroExpander for Generator {
    fn expand<'a>(
        &self,
        cx: &'a mut ExtCtxt,
        _: Span,
        tokens: &[TokenTree],
    ) -> Box<MacResult + 'a> {
        self.data.borrow_mut().expand_foreigner_class(cx, tokens)
    }
}

struct EnumHandler(Rc<RefCell<GeneratorData>>);

impl TTMacroExpander for EnumHandler {
    fn expand<'a>(
        &self,
        cx: &'a mut ExtCtxt,
        _: Span,
        tokens: &[TokenTree],
    ) -> Box<MacResult + 'a> {
        self.0.borrow_mut().expand_foreign_enum(cx, tokens)
    }
}

struct InterfaceHandler(Rc<RefCell<GeneratorData>>);
impl TTMacroExpander for InterfaceHandler {
    fn expand<'a>(
        &self,
        cx: &'a mut ExtCtxt,
        _: Span,
        tokens: &[TokenTree],
    ) -> Box<MacResult + 'a> {
        self.0.borrow_mut().expand_foreign_interface(cx, tokens)
    }
}

impl GeneratorData {
    #[allow(deprecated)]
    fn expand_foreign_interface<'a>(
        &mut self,
        cx: &'a mut ExtCtxt,
        tokens: &[TokenTree],
    ) -> Box<MacResult + 'a> {
        let pointer_target_width = self.pointer_target_width;
        let mut items = unwrap_presult!(
            self.init_types_map(cx.parse_sess(), pointer_target_width),
            self.conv_map
        );
        let foreign_interface =
            parse_foreign_interface(cx, tokens).expect("Can not parse foreign_interface");
        match self.config {
            LanguageConfig::Java {
                ref output_dir,
                ref package_name,
            } => {
                let java_cfg = JavaConfig::new(output_dir.clone(), package_name.clone());
                let mut gen_items = unwrap_presult!(
                    java_cfg.generate_interface(
                        cx.parse_sess(),
                        &mut self.conv_map,
                        self.pointer_target_width,
                        &foreign_interface
                    ),
                    self.conv_map
                );
                items.append(&mut gen_items);
                MacEager::items(SmallVector::many(items))
            }
            LanguageConfig::JavaConfig(ref java_cfg) => {
                let mut gen_items = unwrap_presult!(
                    java_cfg.generate_interface(
                        cx.parse_sess(),
                        &mut self.conv_map,
                        self.pointer_target_width,
                        &foreign_interface
                    ),
                    self.conv_map
                );
                items.append(&mut gen_items);
                MacEager::items(SmallVector::many(items))
            }
        }
    }

    #[allow(deprecated)]
    fn expand_foreign_enum<'a>(
        &mut self,
        cx: &'a mut ExtCtxt,
        tokens: &[TokenTree],
    ) -> Box<MacResult + 'a> {
        let pointer_target_width = self.pointer_target_width;
        let mut items = unwrap_presult!(
            self.init_types_map(cx.parse_sess(), pointer_target_width),
            self.conv_map
        );
        let foreign_enum = parse_foreign_enum(cx, tokens).expect("Can not parse foreign_enum");

        match self.config {
            LanguageConfig::Java {
                ref output_dir,
                ref package_name,
            } => {
                let java_cfg = JavaConfig::new(output_dir.clone(), package_name.clone());
                let mut gen_items = unwrap_presult!(
                    java_cfg.generate_enum(
                        cx.parse_sess(),
                        &mut self.conv_map,
                        self.pointer_target_width,
                        &foreign_enum
                    ),
                    self.conv_map
                );
                items.append(&mut gen_items);
                MacEager::items(SmallVector::many(items))
            }
            LanguageConfig::JavaConfig(ref java_cfg) => {
                let mut gen_items = unwrap_presult!(
                    java_cfg.generate_enum(
                        cx.parse_sess(),
                        &mut self.conv_map,
                        self.pointer_target_width,
                        &foreign_enum
                    ),
                    self.conv_map
                );
                items.append(&mut gen_items);
                MacEager::items(SmallVector::many(items))
            }
        }
    }

    #[allow(deprecated)]
    fn expand_foreigner_class<'a>(
        &mut self,
        cx: &'a mut ExtCtxt,
        tokens: &[TokenTree],
    ) -> Box<MacResult + 'a> {
        let pointer_target_width = self.pointer_target_width;
        let mut items = unwrap_presult!(
            self.init_types_map(cx.parse_sess(), pointer_target_width),
            self.conv_map
        );
        let foreigner_class = match parse_foreigner_class(cx, tokens) {
            Ok(x) => x,
            Err(_) => {
                panic!("Can not parse foreigner_class");
                //return DummyResult::any(span);
            }
        };
        self.conv_map.register_foreigner_class(&foreigner_class);
        match self.config {
            LanguageConfig::Java {
                ref output_dir,
                ref package_name,
            } => {
                let java_cfg = JavaConfig::new(output_dir.clone(), package_name.clone());
                let mut gen_items = unwrap_presult!(
                    java_cfg.generate(
                        cx.parse_sess(),
                        &mut self.conv_map,
                        self.pointer_target_width,
                        &foreigner_class,
                    ),
                    self.conv_map
                );
                items.append(&mut gen_items);
                MacEager::items(SmallVector::many(items))
            }
            LanguageConfig::JavaConfig(ref java_cfg) => {
                let mut gen_items = unwrap_presult!(
                    java_cfg.generate(
                        cx.parse_sess(),
                        &mut self.conv_map,
                        self.pointer_target_width,
                        &foreigner_class
                    ),
                    self.conv_map
                );
                items.append(&mut gen_items);
                MacEager::items(SmallVector::many(items))
            }
        }
    }

    fn init_types_map<'a>(
        &mut self,
        sess: &'a ParseSess,
        target_pointer_width: usize,
    ) -> PResult<'a, Vec<P<ast::Item>>> {
        if self.init_done {
            return Ok(vec![]);
        }
        self.init_done = true;
        for code in &self.conv_map_source {
            self.conv_map
                .merge(sess, code.id_of_code, code.code, target_pointer_width)?;
        }

        if self.conv_map.is_empty() {
            return Err(fatal_error(
                sess,
                DUMMY_SP,
                "After merge all types maps with have no convertion code",
            ));
        }

        Ok(self.conv_map.take_utils_code())
    }
}

/// Configuration for Java
pub struct JavaConfig {
    output_dir: PathBuf,
    package_name: String,
    use_null_annotation: Option<String>,
}

impl JavaConfig {
    /// Create `JavaConfig`
    /// # Arguments
    /// * `output_dir` - directory where place generated java files
    /// * `package_name` - package name for generated java files
    pub fn new(output_dir: PathBuf, package_name: String) -> JavaConfig {
        JavaConfig {
            output_dir,
            package_name,
            use_null_annotation: None,
        }
    }
    /// Use @NonNull for types where appropriate
    /// # Arguments
    /// * `import_annotation` - import statement for @NonNull,
    ///                         for example android.support.annotation.NonNull
    pub fn use_null_annotation(mut self, import_annotation: String) -> JavaConfig {
        self.use_null_annotation = Some(import_annotation);
        self
    }
}
