use lazy_static::lazy_static;
use log::{debug, trace};
use proc_macro2::{Span, TokenStream};
use quote::quote;
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;
use syn::{spanned::Spanned, Ident, Type};

use crate::{
    error::{panic_on_syn_error, DiagnosticError, Result},
    java_jni::{
        calc_this_type_for_method, java_class_full_name, java_class_name_to_jni, method_name,
        ForeignTypeInfo, JavaForeignTypeInfo, JniForeignMethodSignature,
    },
    namegen::new_unique_name,
    source_registry::SourceId,
    typemap::ast::{list_lifetimes, normalize_ty_lifetimes, DisplayToTokens},
    typemap::{
        ty::RustType,
        utils::{
            convert_to_heap_pointer, create_suitable_types_for_constructor_and_self,
            foreign_from_rust_convert_method_output, foreign_to_rust_convert_method_inputs,
            rust_to_foreign_convert_method_inputs, unpack_from_heap_pointer,
        },
        TO_VAR_TEMPLATE,
    },
    types::{
        ForeignInterface, ForeignerClassInfo, ForeignerMethod, MethodVariant, SelfTypeVariant,
    },
    TypeMap, WRITE_TO_MEM_FAILED_MSG,
};

struct MethodContext<'a> {
    class: &'a ForeignerClassInfo,
    method: &'a ForeignerMethod,
    f_method: &'a JniForeignMethodSignature,
    jni_func_name: &'a str,
    decl_func_args: &'a str,
    real_output_typename: &'a str,
    ret_name: &'a str,
}

pub(in crate::java_jni) fn generate_rust_code(
    conv_map: &mut TypeMap,
    package_name: &str,
    class: &ForeignerClassInfo,
    f_methods_sign: &[JniForeignMethodSignature],
) -> Result<Vec<TokenStream>> {
    //to handle java method overload
    let mut gen_fnames = FxHashMap::<String, usize>::default();
    for (method, f_method) in class.methods.iter().zip(f_methods_sign.iter()) {
        let val_ref = gen_fnames.entry(method_name(method, f_method));
        *val_ref.or_insert(0) += 1;
    }

    let dummy_ty = parse_type! { () };
    let dummy_rust_ty = conv_map.find_or_alloc_rust_type_no_src_id(&dummy_ty);

    let mut gen_code = Vec::<TokenStream>::new();
    let (this_type_for_method, code_box_this) =
        if let Some(this_type) = calc_this_type_for_method(conv_map, class) {
            let this_type = conv_map.ty_to_rust_type(&this_type);
            debug!(
                "generate_rust_code: add implements SwigForeignClass for {}",
                this_type.normalized_name
            );

            let (this_type_for_method, code_box_this) =
                convert_to_heap_pointer(conv_map, &this_type, "this");
            let class_name_for_user = java_class_full_name(package_name, &class.name.to_string());
            let class_name_for_jni = java_class_name_to_jni(&class_name_for_user);
            let lifetimes = {
                let mut ret = String::new();
                let lifetimes = list_lifetimes(&this_type.ty);
                for (i, l) in lifetimes.iter().enumerate() {
                    ret.push_str(&*l.as_str());
                    if i != lifetimes.len() - 1 {
                        ret.push(',');
                    }
                }
                ret
            };

            let unpack_code = unpack_from_heap_pointer(&this_type, TO_VAR_TEMPLATE, true);

            let fclass_impl_code = format!(
                r#"impl<{lifetimes}> SwigForeignClass for {class_name} {{
    fn jni_class_name() -> *const ::std::os::raw::c_char {{
        swig_c_str!("{jni_class_name}")
    }}
    fn box_object(this: Self) -> jlong {{
{code_box_this}
       this as jlong
    }}
    fn unbox_object(x: jlong) -> Self {{
        let x: *mut {this_type} = unsafe {{
           jlong_to_pointer::<{this_type}>(x).as_mut().unwrap()
        }};
    {unpack_code}
        x
    }}
}}"#,
                lifetimes = lifetimes,
                class_name = DisplayToTokens(&this_type.ty),
                jni_class_name = class_name_for_jni,
                code_box_this = code_box_this,
                unpack_code = unpack_code.replace(TO_VAR_TEMPLATE, "x"),
                this_type = this_type_for_method.normalized_name,
            );

            gen_code.push(syn::parse_str(&fclass_impl_code).unwrap_or_else(|err| {
                panic_on_syn_error("java internal fclass impl code", fclass_impl_code, err)
            }));

            (this_type_for_method, code_box_this)
        } else {
            (dummy_rust_ty.clone(), String::new())
        };

    let no_this_info = || {
        DiagnosticError::new(
            class.src_id,
            class.span(),
            format!(
                "Class {} (package {}) has methods, but there is no constructor\n
May be you need to use `private constructor = empty;` syntax?",
                class.name, package_name,
            ),
        )
    };

    let mut have_constructor = false;

    for (method, f_method) in class.methods.iter().zip(f_methods_sign.iter()) {
        let java_method_name = method_name(method, f_method);
        let method_overloading = gen_fnames[&java_method_name] > 1;
        let jni_func_name = generate_jni_func_name(
            package_name,
            class,
            &java_method_name,
            method.variant,
            f_method,
            method_overloading,
        )?;
        trace!("generate_rust_code jni name: {}", jni_func_name);

        let mut known_names: FxHashSet<SmolStr> =
            method.arg_names_without_self().map(|x| x.into()).collect();
        if let MethodVariant::Method(_) = method.variant {
            if known_names.contains("this") {
                return Err(DiagnosticError::new(
                    class.src_id,
                    method.rust_id.span(),
                    "Invalid argument name 'this' reserved for generate code purposes",
                ));
            }
            known_names.insert("this".into());
        }
        let ret_name = new_unique_name(&known_names, "ret");
        known_names.insert(ret_name.clone());

        let decl_func_args = {
            use std::fmt::Write;
            let mut buf = String::new();
            for (f_type_info, arg_name) in
                f_method.input.iter().zip(method.arg_names_without_self())
            {
                write!(
                    &mut buf,
                    "{}: {}, ",
                    arg_name,
                    f_type_info.as_ref().correspoding_rust_type.typename()
                )
                .expect(WRITE_TO_MEM_FAILED_MSG);
            }
            buf
        };

        let real_output_typename = match method.fn_decl.output {
            syn::ReturnType::Default => "()",
            syn::ReturnType::Type(_, ref ty) => normalize_ty_lifetimes(&*ty),
        };

        let method_ctx = MethodContext {
            class,
            method,
            f_method,
            jni_func_name: &jni_func_name,
            decl_func_args: &decl_func_args,
            real_output_typename: &real_output_typename,
            ret_name: &ret_name,
        };

        match method.variant {
            MethodVariant::StaticMethod => {
                gen_code.append(&mut generate_static_method(conv_map, &method_ctx)?);
            }
            MethodVariant::Method(ref self_variant) => {
                gen_code.append(&mut generate_method(
                    conv_map,
                    &method_ctx,
                    *self_variant,
                    &this_type_for_method,
                )?);
            }
            MethodVariant::Constructor => {
                have_constructor = true;
                if !method.is_dummy_constructor() {
                    let constructor_ret_type = class
                        .self_desc
                        .as_ref()
                        .map(|x| &x.constructor_ret_type)
                        .ok_or_else(&no_this_info)?
                        .clone();
                    let this_type =
                        calc_this_type_for_method(conv_map, class).ok_or_else(&no_this_info)?;
                    gen_code.append(&mut generate_constructor(
                        conv_map,
                        &method_ctx,
                        constructor_ret_type,
                        this_type,
                        &code_box_this,
                    )?);
                }
            }
        }
    }

    if have_constructor {
        let this_type: RustType = conv_map.find_or_alloc_rust_type(
            &calc_this_type_for_method(conv_map, class).ok_or_else(&no_this_info)?,
            class.src_id,
        );
        let jlong_type = conv_map.ty_to_rust_type(&parse_type! { jlong });

        let unpack_code = unpack_from_heap_pointer(&this_type, "this", false);

        let jni_destructor_name = generate_jni_func_name(
            package_name,
            class,
            "do_delete",
            MethodVariant::StaticMethod,
            &JniForeignMethodSignature {
                output: ForeignTypeInfo {
                    name: "".into(),
                    correspoding_rust_type: dummy_rust_ty.clone(),
                }
                .into(),
                input: vec![JavaForeignTypeInfo {
                    base: ForeignTypeInfo {
                        name: "long".into(),
                        correspoding_rust_type: jlong_type,
                    },
                    java_converter: None,
                    annotation: None,
                }],
            },
            false,
        )?;
        let code = format!(
            r#"
#[allow(unused_variables, unused_mut, non_snake_case, unused_unsafe)]
#[no_mangle]
pub extern "C" fn {jni_destructor_name}(env: *mut JNIEnv, _: jclass, this: jlong) {{
    let this: *mut {this_type} = unsafe {{
        jlong_to_pointer::<{this_type}>(this).as_mut().unwrap()
    }};
{unpack_code}
    drop(this);
}}
"#,
            jni_destructor_name = jni_destructor_name,
            unpack_code = unpack_code,
            this_type = this_type_for_method.normalized_name,
        );
        debug!("we generate and parse code: {}", code);
        gen_code.push(
            syn::parse_str(&code).unwrap_or_else(|err| {
                panic_on_syn_error("java/jni internal desctructor", code, err)
            }),
        );
    }

    Ok(gen_code)
}

pub(in crate::java_jni) fn generate_interface(
    package_name: &str,
    conv_map: &mut TypeMap,
    pointer_target_width: usize,
    interface: &ForeignInterface,
    methods_sign: &[JniForeignMethodSignature],
) -> Result<Vec<TokenStream>> {
    use std::fmt::Write;

    let mut new_conv_code = format!(
        r#"
#[swig_from_foreigner_hint = "{interface_name}"]
impl SwigFrom<jobject> for Box<{trait_name}> {{
    fn swig_from(this: jobject, env: *mut JNIEnv) -> Self {{
        let mut cb = JavaCallback::new(this, env);
        cb.methods.reserve({methods_len});
        let class = unsafe {{ (**env).GetObjectClass.unwrap()(env, cb.this) }};
        assert!(!class.is_null(), "GetObjectClass return null class for {interface_name}");
"#,
        interface_name = interface.name,
        trait_name = DisplayToTokens(&interface.self_type),
        methods_len = interface.items.len(),
    );
    for (method, f_method) in interface.items.iter().zip(methods_sign) {
        write!(
            &mut new_conv_code,
            r#"
        let method_id: jmethodID = unsafe {{
            (**env).GetMethodID.unwrap()(env, class, swig_c_str!("{method_name}"),
                                         swig_c_str!("{method_sig}"))
        }};
        assert!(!method_id.is_null(), "Can not find {method_name} id");
        cb.methods.push(method_id);
"#,
            method_name = method.name,
            method_sig = jni_method_signature(f_method, package_name, conv_map),
        )
        .unwrap();
    }
    write!(
        &mut new_conv_code,
        r#"
        Box::new(cb)
    }}
}}
"#
    )
    .unwrap();
    conv_map.merge(SourceId::none(), &new_conv_code, pointer_target_width)?;

    let mut trait_impl_funcs = Vec::<TokenStream>::with_capacity(interface.items.len());

    let mut gen_items = Vec::with_capacity(1);

    for (method_idx, (method, f_method)) in interface.items.iter().zip(methods_sign).enumerate() {
        let func_name = &method
            .rust_name
            .segments
            .last()
            .ok_or_else(|| {
                DiagnosticError::new(
                    interface.src_id,
                    method.rust_name.span(),
                    "Empty trait function name",
                )
            })?
            .value()
            .ident;

        let self_arg: TokenStream = method.fn_decl.inputs[0]
            .as_self_arg(interface.src_id)?
            .into();
        let mut args_with_types = Vec::with_capacity(method.fn_decl.inputs.len());
        args_with_types.push(self_arg);
        args_with_types.extend(
            method
                .fn_decl
                .inputs
                .iter()
                .skip(1)
                .enumerate()
                .map(|(i, v)| {
                    let arg_ty = &v.as_named_arg().unwrap().ty;
                    let arg_name = Ident::new(&format!("a{}", i), Span::call_site());
                    quote!(#arg_name: #arg_ty)
                }),
        );
        assert!(!method.fn_decl.inputs.is_empty());
        let n_args = method.fn_decl.inputs.len() - 1;
        let (args, type_size_asserts) = convert_args_for_variadic_function_call(f_method);

        let (mut conv_deps, convert_args_code) = rust_to_foreign_convert_method_inputs(
            conv_map,
            interface.src_id,
            method,
            f_method,
            (0..n_args).map(|v| format!("a{}", v)),
            "()",
        )?;
        gen_items.append(&mut conv_deps);
        let convert_args: TokenStream = syn::parse_str(&convert_args_code).unwrap_or_else(|err| {
            panic_on_syn_error(
                "java/jni internal parse failed for convert arguments code",
                convert_args_code,
                err,
            )
        });

        trait_impl_funcs.push(quote! {
            #[allow(unused_mut)]
            fn #func_name(#(#args_with_types),*) {
                #type_size_asserts
                let env = self.get_jni_env();
                if let Some(env) = env.env {
                    #convert_args
                    unsafe {
                        (**env).CallVoidMethod.unwrap()(env, self.this,
                                                        self.methods[#method_idx],
                                                        #(#args),*);
                        if (**env).ExceptionCheck.unwrap()(env) != 0 {
                            error!(concat!(stringify!(#func_name), ": java throw exception"));
                            (**env).ExceptionDescribe.unwrap()(env);
                            (**env).ExceptionClear.unwrap()(env);
                        }
                    };
                }
            }
        });
    }

    let self_type_name = &interface.self_type;
    let tt: TokenStream = quote! {
        impl #self_type_name for JavaCallback {
            #(#trait_impl_funcs)*
        }
    };
    gen_items.push(tt);
    Ok(gen_items)
}

lazy_static! {
    static ref JAVA_TYPE_NAMES_FOR_JNI_SIGNATURE: FxHashMap<&'static str, &'static str> = {
        let mut m = FxHashMap::default();
        m.insert("String", "Ljava.lang.String;");
        m.insert("Integer", "Ljava.lang.Integer");
        m.insert("Long", "Ljava.lang.Long");
        m.insert("Double", "Ljava.lang.Double");
        m.insert("boolean", "Z");
        m.insert("byte", "B");
        m.insert("char", "C");
        m.insert("double", "D");
        m.insert("float", "F");
        m.insert("int", "I");
        m.insert("long", "J");
        m.insert("object", "L");
        m.insert("short", "S");
        m.insert("void", "V");
        m
    };
    static ref JNI_FOR_VARIADIC_C_FUNC_CALL: FxHashMap<&'static str, &'static str> = {
        let mut m = FxHashMap::default();
        m.insert("jboolean", "::std::os::raw::c_uint");
        m.insert("jbyte", "::std::os::raw::c_int");
        m.insert("jshort", "::std::os::raw::c_int");
        m.insert("jfloat", "f64");
        m
    };
}

fn generate_jni_func_name(
    package_name: &str,
    class: &ForeignerClassInfo,
    java_method_name: &str,
    method_type: MethodVariant,
    f_method: &JniForeignMethodSignature,
    overloaded: bool,
) -> Result<String> {
    let mut output = String::new();
    output.push_str("Java_");
    fn escape_underscore(input: &str, output: &mut String) {
        for c in input.chars() {
            match c {
                '.' => output.push('_'),
                '[' => output.push_str("_3"),
                '_' => output.push_str("_1"),
                ';' => output.push_str("_2"),
                _ => output.push(c),
            }
        }
    }
    escape_underscore(package_name, &mut output);
    output.push_str("_");
    escape_underscore(&class.name.to_string(), &mut output);
    output.push_str("_");
    escape_underscore(java_method_name, &mut output);

    if overloaded {
        output.push_str("__");
        if let MethodVariant::Method(_) = method_type {
            output.push('J');
        }
        for arg in &f_method.input {
            let type_name = arg
                .java_converter
                .as_ref()
                .map(|x| x.java_transition_type.as_str())
                .unwrap_or_else(|| arg.as_ref().name.as_str());

            let type_name = JAVA_TYPE_NAMES_FOR_JNI_SIGNATURE
                .get(type_name)
                .ok_or_else(|| {
                    DiagnosticError::new(
                        class.src_id,
                        class.span(),
                        format!(
                            "Can not generate JNI function name for overload method '{}',\
                             unknown java type '{}'",
                            java_method_name,
                            arg.as_ref().name
                        ),
                    )
                })?;

            escape_underscore(type_name, &mut output);
        }
    }

    Ok(output)
}

fn generate_static_method(conv_map: &mut TypeMap, mc: &MethodContext) -> Result<Vec<TokenStream>> {
    let jni_ret_type = mc.f_method.output.base.correspoding_rust_type.typename();
    let (mut deps_code_out, convert_output_code) = foreign_from_rust_convert_method_output(
        conv_map,
        mc.class.src_id,
        &mc.method.fn_decl.output,
        &mc.f_method.output,
        mc.ret_name,
        &jni_ret_type,
    )?;
    let (deps_code_in, convert_input_code) = foreign_to_rust_convert_method_inputs(
        conv_map,
        mc.class.src_id,
        mc.method,
        mc.f_method,
        mc.method.arg_names_without_self(),
        &jni_ret_type,
    )?;

    let code = format!(
        r#"
#[allow(non_snake_case, unused_variables, unused_mut, unused_unsafe)]
#[no_mangle]
pub extern "C" fn {func_name}(env: *mut JNIEnv, _: jclass, {decl_func_args}) -> {jni_ret_type} {{
{convert_input_code}
    let mut {ret_name}: {real_output_typename} = {call};
{convert_output_code}
    {ret_name}
}}
"#,
        func_name = mc.jni_func_name,
        decl_func_args = mc.decl_func_args,
        jni_ret_type = jni_ret_type,
        convert_input_code = convert_input_code,
        convert_output_code = convert_output_code,
        real_output_typename = mc.real_output_typename,
        call = mc.method.generate_code_to_call_rust_func(),
        ret_name = mc.ret_name,
    );
    let mut gen_code = deps_code_in;
    gen_code.append(&mut deps_code_out);
    gen_code
        .push(syn::parse_str(&code).unwrap_or_else(|err| {
            panic_on_syn_error("java/jni internal static method", code, err)
        }));
    Ok(gen_code)
}

fn generate_constructor(
    conv_map: &mut TypeMap,
    mc: &MethodContext,
    construct_ret_type: Type,
    this_type: Type,
    code_box_this: &str,
) -> Result<Vec<TokenStream>> {
    let (deps_code_in, convert_input_code) = foreign_to_rust_convert_method_inputs(
        conv_map,
        mc.class.src_id,
        mc.method,
        mc.f_method,
        mc.method.arg_names_without_self(),
        "jlong",
    )?;

    let this_type = conv_map.ty_to_rust_type(&this_type);
    let construct_ret_type = conv_map.ty_to_rust_type(&construct_ret_type);

    let (mut deps_this, convert_this) = conv_map.convert_rust_types(
        construct_ret_type.to_idx(),
        this_type.to_idx(),
        "this",
        "this",
        "jlong",
        (mc.class.src_id, mc.method.span()),
    )?;

    let code = format!(
        r#"
#[allow(unused_variables, unused_mut, non_snake_case, unused_unsafe)]
#[no_mangle]
pub extern "C" fn {func_name}(env: *mut JNIEnv, _: jclass, {decl_func_args}) -> jlong {{
{convert_input_code}
    let this: {real_output_typename} = {call};
{convert_this}
{box_this}
    this as jlong
}}
"#,
        func_name = mc.jni_func_name,
        convert_this = convert_this,
        decl_func_args = mc.decl_func_args,
        convert_input_code = convert_input_code,
        box_this = code_box_this,
        real_output_typename = mc.real_output_typename,
        call = mc.method.generate_code_to_call_rust_func(),
    );
    let mut gen_code = deps_code_in;
    gen_code.append(&mut deps_this);
    gen_code.push(
        syn::parse_str(&code)
            .unwrap_or_else(|err| panic_on_syn_error("java/jni internal constructor", code, err)),
    );
    Ok(gen_code)
}

fn generate_method(
    conv_map: &mut TypeMap,
    mc: &MethodContext,
    self_variant: SelfTypeVariant,
    this_type_for_method: &RustType,
) -> Result<Vec<TokenStream>> {
    let jni_ret_type = mc.f_method.output.base.correspoding_rust_type.typename();
    let (deps_code_in, convert_input_code) = foreign_to_rust_convert_method_inputs(
        conv_map,
        mc.class.src_id,
        mc.method,
        mc.f_method,
        mc.method.arg_names_without_self(),
        &jni_ret_type,
    )?;

    let (mut deps_code_out, convert_output_code) = foreign_from_rust_convert_method_output(
        conv_map,
        mc.class.src_id,
        &mc.method.fn_decl.output,
        &mc.f_method.output,
        mc.ret_name,
        &jni_ret_type,
    )?;

    //&mut constructor_real_type -> &mut class.self_type

    let (from_ty, to_ty): (Type, Type) = create_suitable_types_for_constructor_and_self(
        self_variant,
        mc.class,
        &this_type_for_method.ty,
    );
    let from_ty = conv_map.find_or_alloc_rust_type(&from_ty, mc.class.src_id);
    let this_type_ref = from_ty.normalized_name.as_str();
    let to_ty = conv_map.find_or_alloc_rust_type(&to_ty, mc.class.src_id);

    let (mut deps_this, convert_this) = conv_map.convert_rust_types(
        from_ty.to_idx(),
        to_ty.to_idx(),
        "this",
        "this",
        jni_ret_type,
        (mc.class.src_id, mc.method.span()),
    )?;

    let code = format!(
        r#"
#[allow(non_snake_case, unused_variables, unused_mut, unused_unsafe)]
#[no_mangle]
pub extern "C"
 fn {func_name}(env: *mut JNIEnv, _: jclass, this: jlong, {decl_func_args}) -> {jni_ret_type} {{
{convert_input_code}
    let this: {this_type_ref} = unsafe {{
        jlong_to_pointer::<{this_type}>(this).as_mut().unwrap()
    }};
{convert_this}
    let mut {ret_name}: {real_output_typename} = {call};
{convert_output_code}
    {ret_name}
}}
"#,
        func_name = mc.jni_func_name,
        decl_func_args = mc.decl_func_args,
        convert_input_code = convert_input_code,
        jni_ret_type = jni_ret_type,
        this_type_ref = this_type_ref,
        this_type = this_type_for_method.normalized_name,
        convert_this = convert_this,
        convert_output_code = convert_output_code,
        real_output_typename = mc.real_output_typename,
        call = mc.method.generate_code_to_call_rust_func(),
        ret_name = mc.ret_name,
    );
    let mut gen_code = deps_code_in;
    gen_code.append(&mut deps_code_out);
    gen_code.append(&mut deps_this);
    gen_code.push(
        syn::parse_str(&code)
            .unwrap_or_else(|err| panic_on_syn_error("java/jni internal method", code, err)),
    );
    Ok(gen_code)
}

fn jni_method_signature(
    method: &JniForeignMethodSignature,
    package_name: &str,
    conv_map: &TypeMap,
) -> String {
    let mut ret: String = "(".into();
    for arg in &method.input {
        let mut gen_sig = String::new();
        let sig = JAVA_TYPE_NAMES_FOR_JNI_SIGNATURE
            .get(&arg.as_ref().name.as_str())
            .cloned()
            .or_else(|| {
                if conv_map.is_generated_foreign_type(&arg.as_ref().name) {
                    gen_sig = format!(
                        "L{};",
                        &java_class_full_name(package_name, &*arg.as_ref().name.as_str())
                    );
                    Some(&gen_sig)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                panic!(
                    "Unknown type `{}`, can not generate jni signature",
                    arg.as_ref().name
                )
            });
        let sig = sig.replace('.', "/");
        ret.push_str(&sig);
    }
    ret.push(')');
    let sig = JAVA_TYPE_NAMES_FOR_JNI_SIGNATURE
        .get(&*method.output.base.name.as_str())
        .unwrap_or_else(|| {
            panic!(
                "Unknown type `{}`, can not generate jni signature",
                method.output.base.name
            )
        });
    ret.push_str(sig);
    ret
}

// To use `C` function with variable number of arguments,
// we need automatic type conversation, see
// http://en.cppreference.com/w/c/language/conversion#Default_argument_promotions
// for more details.
// return arg with conversation plus asserts
fn convert_args_for_variadic_function_call(
    f_method: &JniForeignMethodSignature,
) -> (Vec<TokenStream>, TokenStream) {
    let mut ret = Vec::with_capacity(f_method.input.len());
    for (i, arg) in f_method.input.iter().enumerate() {
        let arg_name = Ident::new(&format!("a{}", i), Span::call_site());
        if let Some(conv_type_str) = JNI_FOR_VARIADIC_C_FUNC_CALL
            .get(&*arg.as_ref().correspoding_rust_type.normalized_name.as_str())
        {
            let conv_type: TokenStream = syn::parse_str(*conv_type_str).unwrap_or_else(|err| {
                panic_on_syn_error(
                    "java/jni internal error: can not parse type for variable conversation",
                    conv_type_str.to_string(),
                    err,
                )
            });
            ret.push(quote!(#arg_name as #conv_type));
        } else {
            ret.push(quote!(#arg_name));
        }
    }
    let check_sizes = quote! {
        swig_assert_eq_size!(::std::os::raw::c_uint, u32);
        swig_assert_eq_size!(::std::os::raw::c_int, i32);
    };
    (ret, check_sizes)
}
