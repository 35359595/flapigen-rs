use crate::{
    DiagnosticError, error::Result, ForeignEnumInfo, ForeignInterface, ForeignerClassInfo, ForeignerMethod, LanguageGenerator,
    MethodVariant, PythonConfig, TypeMap,
};
use heck::SnakeCase;
use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

fn method_name(method: &ForeignerMethod) -> Result<&syn::Ident> {
    Ok(&method.rust_id.segments.last().ok_or_else(|| {
        DiagnosticError::new(method.span(), "Method has no name")
    })?.value().ident)
}

impl LanguageGenerator for PythonConfig {
    fn register_class(&self, conv_map: &mut TypeMap, class: &ForeignerClassInfo) -> Result<()> {
        Ok(())
    }

    fn generate(
        &self,
        conv_map: &mut TypeMap,
        _: usize,
        class: &ForeignerClassInfo,
    ) -> Result<Vec<TokenStream>> {
        let class_name = &class.name;
        let wrapper_mod_name = syn::parse_str::<Ident>(&format!(
            "py_{}",
            class_name.to_string().to_snake_case()
        ))?;
        self.module_initialization_code.borrow_mut().push(quote! {
            {
                m.add_class::<#wrapper_mod_name::#class_name>(py)?;
            }
        });
        // let method_names = class.methods
        //     .iter()
        //     .filter(|m| m.variant == MethodVariant::Method)
        //     .map(method_name)
        //     .collect::<Result<Vec<_>>>()?;
        // let method_rust_paths = class.methods
        //     .iter()
        //     .filter(|m| m.variant == MethodVariant::Method)
        //     .map(|m| m.rust_id);
        let static_method_names = class.methods
            .iter()
            .filter(|m| m.variant == MethodVariant::StaticMethod)
            .map(method_name)
            .collect::<Result<Vec<_>>>()?;
        let static_method_rust_paths = class.methods
            .iter()
            .filter(|m| m.variant == MethodVariant::StaticMethod)
            .map(|m| &m.rust_id);
        let self_type = class.self_type_as_ty();
        let rust_instance_field_code = generate_rust_instance_field_code(class, conv_map)?;
        let constructor_code = generate_constructor_code(class, conv_map)?;
        let class_code = quote! {
            mod #wrapper_mod_name {
                py_class!(pub class #class_name |py| {
                    #rust_instance_field_code
                    
                    #constructor_code

                    #( @staticmethod def #static_method_names() -> cpython::PyResult<cpython::PyObject> {
                        super::#static_method_rust_paths();
                        Ok(py.None()) 
                    } )*
                });
            }
        };
        Ok(vec![class_code])
    }

    fn generate_enum(
        &self,
        conv_map: &mut TypeMap,
        pointer_target_width: usize,
        enum_info: &ForeignEnumInfo,
    ) -> Result<Vec<TokenStream>> {
        let enum_name = &enum_info.name;
        let wrapper_mod_name = syn::parse_str::<Ident>(&format!(
            "py_{}",
            enum_info.name.to_string().to_snake_case()
        ))?;
        self.module_initialization_code.borrow_mut().push(quote! {
            {
                m.add_class::<#wrapper_mod_name::#enum_name>(py)?;
            }

        });
        let foreign_variants = enum_info.items.iter().map(|item| &item.name);
        let rust_variants = enum_info.items.iter().map(|item| &item.rust_name);
        let class_code = quote! {
            mod #wrapper_mod_name {
                py_class!(pub class #enum_name |py| {
                    #( static #foreign_variants = super::#rust_variants as u32; )*
                });
            }
        };
        Ok(vec![class_code])
    }

    fn generate_interface(
        &self,
        conv_map: &mut TypeMap,
        pointer_target_width: usize,
        interface: &ForeignInterface,
    ) -> Result<Vec<TokenStream>> {
        Ok(vec![])
    }

    fn init_glue_rs(&self, _conv_map: &mut TypeMap) -> Result<Vec<TokenStream>> {
        Ok(vec![quote! {
            #[macro_use] extern crate cpython;
            use cpython::{ObjectProtocol};
        }])
    }

    fn finish_glue_rs(&self, _conv_map: &mut TypeMap) -> Result<Vec<TokenStream>> {
        let module_initialization_code_cell = self.module_initialization_code.borrow();
        let module_initialization_code = &*module_initialization_code_cell;
        let registration_code = vec![quote! {

            py_module_initializer!(librust_swig_test_python, initlibrust_swig_test_python, PyInit_rust_swig_test_python, |py, m| {
                m.add(py, "__doc__", "This is test module for rust_swig.")?;
                #(#module_initialization_code)*
                Ok(())
            });
        }];
        Ok(registration_code)
    }
}

fn generate_rust_instance_field_code(class: &ForeignerClassInfo, conv_map: &TypeMap) -> Result<TokenStream> {
    if let Some(ref rust_self_type) = class.self_type {
        let self_type = &rust_self_type.ty;
        Ok(quote!{
            data rust_instance: std::sync::RwLock<super::#self_type>;
        })
    } else if !has_any_methods(class) {
        Ok(TokenStream::new())
    } else {
        Err(DiagnosticError::new(class.span(), format!("Class {} has non-static methods, but no self_type", class.name)))
    }
}

fn generate_constructor_code(class: &ForeignerClassInfo, conv_map: &TypeMap) -> Result<TokenStream> {
    if let Some(constructor) = class.methods
        .iter()
        .find(|m| m.variant == MethodVariant::Constructor) {
        let class_name = &class.name;
        let constructor_rust_path = &constructor.rust_id;
        Ok(quote!{
             def __new__(_cls) -> cpython::PyResult<#class_name> {
                let rust_instance = super::#constructor_rust_path();
                #class_name::create_instance(py, std::sync::RwLock::new(rust_instance))
            }
        })
    } else if !has_any_methods(class) {
        Ok(TokenStream::new())
    } else {
        Err(DiagnosticError::new(class.span(), format!("Class {} has non-static methods, but no constructor", class.name)))
    }
}

fn has_any_methods(class: &ForeignerClassInfo) -> bool {
    class.methods
        .iter()
        .any(|m| if let MethodVariant::Method(_) = m.variant { true } else { false })
}