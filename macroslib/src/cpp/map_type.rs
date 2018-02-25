use syntex_syntax::parse::{PResult, ParseSess};
use syntex_syntax::ast;
use syntex_pos::{Span, DUMMY_SP};
use syntex_syntax::symbol::Symbol;
use petgraph::Direction;

use my_ast::{if_option_return_some_type, if_result_return_ok_err_types, normalized_ty_string,
             parse_ty, RustType};
use errors::fatal_error;
use types_conv_map::{ForeignTypeInfo, FROM_VAR_TEMPLATE};
use {CppConfig, CppOptional, CppVariant, ForeignEnumInfo, ForeignerClassInfo, TypesConvMap};
use cpp::{CppConverter, CppForeignTypeInfo};
use cpp::cpp_code::c_class_type;

fn special_type<'a>(
    sess: &'a ParseSess,
    conv_map: &mut TypesConvMap,
    cpp_cfg: &CppConfig,
    arg_ty: &ast::Ty,
    direction: Direction,
) -> PResult<'a, Option<CppForeignTypeInfo>> {
    trace!(
        "special_type: begin arg.ty({:?}) input {:?}",
        arg_ty,
        direction
    );

    if let Some(foreign_enum) = conv_map.is_this_exported_enum(arg_ty) {
        let converter = calc_converter_for_enum(foreign_enum);
        return Ok(Some(converter));
    }

    let ty_name = normalized_ty_string(arg_ty);
    if ty_name == "bool" {
        let fti = conv_map
            .find_foreign_type_info_by_name(Symbol::intern("char"))
            .expect("expect find char in type map");
        return Ok(Some(CppForeignTypeInfo {
            base: fti,
            c_converter: String::new(),
            cpp_converter: Some(CppConverter {
                typename: Symbol::intern("bool"),
                input_converter: format!("{} ? 1 : 0", FROM_VAR_TEMPLATE),
                output_converter: format!("{} != 0", FROM_VAR_TEMPLATE),
            }),
        }));
    }

    if let ast::TyKind::Rptr(
        _,
        ast::MutTy {
            ty: ref ret_ty,
            mutbl: ast::Mutability::Immutable,
        },
    ) = arg_ty.node
    {
        if let Some(foreign_class) =
            conv_map.find_foreigner_class_with_such_self_type(ret_ty, false)
        {
            let foreign_info =
                foreign_class_foreign_name(sess, conv_map, foreign_class, arg_ty.span, true)?;
            if direction == Direction::Outgoing {
                let cpp_type = Symbol::intern(&format!("{}Ref", foreign_class.name));
                return Ok(Some(CppForeignTypeInfo {
                    base: foreign_info,
                    c_converter: String::new(),
                    cpp_converter: Some(CppConverter {
                        typename: cpp_type,
                        output_converter: format!("{}{{{}}}", cpp_type, FROM_VAR_TEMPLATE),
                        input_converter: "UNREACHABLE".to_string(),
                    }),
                }));
            } else {
                let cpp_type = Symbol::intern(&format!("const {} &", foreign_class.name));
                let c_type = foreign_info.name;
                return Ok(Some(CppForeignTypeInfo {
                    base: foreign_info,
                    c_converter: String::new(),
                    cpp_converter: Some(CppConverter {
                        typename: cpp_type,
                        output_converter: "UNREACHABLE".to_string(),
                        input_converter: format!("static_cast<{}>({})", c_type, FROM_VAR_TEMPLATE),
                    }),
                }));
            }
        }
    }

    if let Some(foreign_class) = conv_map.find_foreigner_class_with_such_self_type(arg_ty, false) {
        trace!(
            "special_type: {:?} is foreign_class {}",
            arg_ty,
            foreign_class.name
        );
        let foreign_info =
            foreign_class_foreign_name(sess, conv_map, foreign_class, arg_ty.span, false)?;
        return Ok(Some(CppForeignTypeInfo {
            base: foreign_info,
            c_converter: String::new(),
            cpp_converter: Some(CppConverter {
                typename: foreign_class.name,
                output_converter: format!("{}({})", foreign_class.name, FROM_VAR_TEMPLATE),
                input_converter: format!("{}.release()", FROM_VAR_TEMPLATE),
            }),
        }));
    }

    if direction == Direction::Outgoing {
        if let Some((ok_ty, err_ty)) = if_result_return_ok_err_types(arg_ty) {
            trace!(
                "special_type: return type is Result<{:?}, {:?}>",
                ok_ty,
                err_ty
            );
            if let Some(foreign_class) =
                conv_map.find_foreigner_class_with_such_self_type(&ok_ty, false)
            {
                let foreign_info = conv_map
                    .find_foreign_type_info_by_name(Symbol::intern("struct CResultObjectString"))
                    .expect("Can not find info about struct CResultObjectString");
                let err_ty_name = normalized_ty_string(&err_ty);
                if err_ty_name == "String" {
                    let c_class = c_class_type(foreign_class);
                    let typename = match cpp_cfg.cpp_variant {
                        CppVariant::Std17 => Symbol::intern(&format!(
                            "std::variant<{}, RustString>",
                            foreign_class.name
                        )),
                        CppVariant::Boost => Symbol::intern(&format!(
                            "boost::variant<{}, RustString>",
                            foreign_class.name
                        )),
                    };
                    let output_converter = format!(
                        "{var}.is_ok != 0 ?
 {VarType}{{{Type}(static_cast<{C_Type} *>({var}.data.ok))}} :
 {VarType}{{RustString{{{var}.data.err}}}}",
                        VarType = typename,
                        Type = foreign_class.name,
                        C_Type = c_class,
                        var = FROM_VAR_TEMPLATE,
                    );
                    return Ok(Some(CppForeignTypeInfo {
                        base: foreign_info,
                        c_converter: String::new(),
                        cpp_converter: Some(CppConverter {
                            typename,
                            output_converter,
                            input_converter: String::new(),
                        }),
                    }));
                } else {
                    unimplemented!();
                }
            //if let Some(foreign_class)
            } else {
                trace!("return result, but not foreign_class");
                match ok_ty.node {
                    //Result<(), err_ty>
                    ast::TyKind::Tup(ref tup_types) if tup_types.is_empty() => {
                        let err_ty_name = normalized_ty_string(&err_ty);
                        if err_ty_name == "String" {
                            let typename = match cpp_cfg.cpp_variant {
                                CppVariant::Std17 => {
                                    Symbol::intern("std::variant<void *, RustString>")
                                }
                                CppVariant::Boost => {
                                    Symbol::intern("boost::variant<void *, RustString>")
                                }
                            };
                            let output_converter = format!(
                                "{var}.is_ok != 0 ?
 {VarType}{{{var}.data.ok}} :
 {VarType}{{RustString{{{var}.data.err}}}}",
                                VarType = typename,
                                var = FROM_VAR_TEMPLATE
                            );
                            let foreign_info = conv_map
                                .find_foreign_type_info_by_name(Symbol::intern(
                                    "struct CResultObjectString",
                                ))
                                .expect("Can not find struct CResultObjectString");
                            return Ok(Some(CppForeignTypeInfo {
                                base: foreign_info,
                                c_converter: String::new(),
                                cpp_converter: Some(CppConverter {
                                    typename,
                                    output_converter,
                                    input_converter: String::new(),
                                }),
                            }));
                        } else {
                            unimplemented!();
                        }
                    }
                    _ => unimplemented!(),
                }
            }
        }
        if let Some(ty) = if_option_return_some_type(arg_ty) {
            if let Some(foreign_class) =
                conv_map.find_foreigner_class_with_such_self_type(&ty, false)
            {
                let foreign_info =
                    foreign_class_foreign_name(sess, conv_map, foreign_class, ty.span, false)?;
                let (typename, output_converter) = match cpp_cfg.cpp_optional {
                    CppOptional::Std17 => (
                        Symbol::intern(&format!("std::optional<{}>", foreign_class.name)),
                        format!(
                            "{var} != nullptr ? {Type}({var}) : std::optional<{Type}>()",
                            Type = foreign_class.name,
                            var = FROM_VAR_TEMPLATE,
                        ),
                    ),
                    CppOptional::Boost => (
                        Symbol::intern(&format!("boost::optional<{}>", foreign_class.name)),
                        format!(
                            "{var} != nullptr ? {Type}({var}) : boost::optional<{Type}>()",
                            Type = foreign_class.name,
                            var = FROM_VAR_TEMPLATE,
                        ),
                    ),
                };
                return Ok(Some(CppForeignTypeInfo {
                    base: foreign_info,
                    c_converter: String::new(),
                    cpp_converter: Some(CppConverter {
                        typename,
                        output_converter,
                        input_converter: String::new(),
                    }),
                }));
            } else {
                unimplemented!();
            }
        }
    }

    trace!("Oridinary type {:?}", arg_ty);
    Ok(None)
}

fn foreign_class_foreign_name<'a>(
    sess: &'a ParseSess,
    conv_map: &TypesConvMap,
    foreign_class: &ForeignerClassInfo,
    foreign_class_span: Span,
    readonly_fptr: bool,
) -> PResult<'a, ForeignTypeInfo> {
    let c_type = c_class_type(foreign_class);
    let foreign_typename = Symbol::intern(&if readonly_fptr {
        format!("const {} *", c_type)
    } else {
        format!("{} *", c_type)
    });
    conv_map
        .find_foreign_type_info_by_name(foreign_typename)
        .ok_or_else(|| {
            fatal_error(
                sess,
                foreign_class_span,
                &format!("type {} unknown", foreign_class.name),
            )
        })
}

fn calc_converter_for_enum(foreign_enum: &ForeignEnumInfo) -> CppForeignTypeInfo {
    let sess = ParseSess::new();
    let u32_ti: RustType = parse_ty(&sess, DUMMY_SP, Symbol::intern("u32"))
        .unwrap()
        .into();
    let c_converter: String = r#"
        uint32_t {to_var} = {from_var};
"#.into();
    CppForeignTypeInfo {
        base: ForeignTypeInfo {
            name: foreign_enum.name,
            correspoding_rust_type: u32_ti,
        },
        c_converter,
        cpp_converter: None,
    }
}

pub(in cpp) fn map_type<'a>(
    sess: &'a ParseSess,
    conv_map: &mut TypesConvMap,
    cpp_cfg: &CppConfig,
    arg_ty: &ast::Ty,
    direction: Direction,
) -> PResult<'a, CppForeignTypeInfo> {
    let ret: CppForeignTypeInfo = match direction {
        Direction::Incoming => {
            if let Some(converter) =
                special_type(sess, conv_map, cpp_cfg, arg_ty, Direction::Incoming)?
            {
                return Ok(converter);
            }
            let f_arg_type = conv_map
                .map_through_conversation_to_foreign(arg_ty, Direction::Incoming, arg_ty.span)
                .ok_or_else(|| {
                    fatal_error(
                        sess,
                        arg_ty.span,
                        &format!(
                            "Do not know conversation from foreign \
                             to such rust type '{}'",
                            normalized_ty_string(arg_ty)
                        ),
                    )
                })?;
            f_arg_type.into()
        }
        Direction::Outgoing => {
            if let Some(converter) =
                special_type(sess, conv_map, cpp_cfg, arg_ty, Direction::Outgoing)?
            {
                converter
            } else {
                conv_map
                    .map_through_conversation_to_foreign(arg_ty, Direction::Outgoing, arg_ty.span)
                    .ok_or_else(|| {
                        fatal_error(
                            sess,
                            arg_ty.span,
                            &format!(
                                "Do not know conversation from \
                                 such rust type '{}' to foreign",
                                normalized_ty_string(arg_ty)
                            ),
                        )
                    })?
                    .into()
            }
        }
    };
    Ok(ret)
}
