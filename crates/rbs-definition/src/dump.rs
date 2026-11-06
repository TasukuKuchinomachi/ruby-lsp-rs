use std::{fmt::Write, path::Path};

use crate::{
    ast::{
        AttributeKind, ClassOrModule, Declaration, Member, MemberKind, MethodKind, MixinKind, Type,
        TypeParameter, VariableKind, Visibility,
    },
    environment::Environment,
};

pub fn dump_environment(environment: &Environment, rbs_gem_directory: &Path) -> String {
    let mut output = String::new();
    for source in &environment.sources {
        let file_name = source
            .path
            .strip_prefix(rbs_gem_directory)
            .unwrap_or(&source.path);
        writeln!(output, "# {}", file_name.display()).unwrap();
        for declaration in &source.declarations {
            dump_declaration(&mut output, declaration, 0);
        }
    }
    output
}

fn type_parameters(parameters: &[TypeParameter]) -> String {
    if parameters.is_empty() {
        String::new()
    } else {
        let parameters: Vec<String> = parameters.iter().map(ToString::to_string).collect();
        format!("[{}]", parameters.join(", "))
    }
}

fn arguments(args: &[Type]) -> String {
    if args.is_empty() {
        String::new()
    } else {
        let args: Vec<String> = args.iter().map(ToString::to_string).collect();
        format!("[{}]", args.join(", "))
    }
}

fn annotations(annotations: &[String]) -> String {
    annotations
        .iter()
        .map(|annotation| format!("%a{{{annotation}}} "))
        .collect()
}

fn visibility(visibility: Option<Visibility>) -> &'static str {
    match visibility {
        None => "",
        Some(Visibility::Public) => "public ",
        Some(Visibility::Private) => "private ",
    }
}

fn dump_declaration(output: &mut String, declaration: &Declaration, depth: usize) {
    let indent = "  ".repeat(depth);
    match declaration {
        Declaration::Class(class) => {
            let super_class = class
                .super_class
                .as_ref()
                .map(|super_class| {
                    format!(" < {}{}", super_class.name, arguments(&super_class.args))
                })
                .unwrap_or_default();
            writeln!(
                output,
                "{indent}{}class {}{}{super_class}",
                annotations(&class.annotations),
                class.name,
                type_parameters(&class.type_parameters)
            )
            .unwrap();
            dump_members(output, &class.members, depth + 1);
            writeln!(output, "{indent}end").unwrap();
        }
        Declaration::Module(module) => {
            let self_types = if module.self_types.is_empty() {
                String::new()
            } else {
                let self_types: Vec<String> = module
                    .self_types
                    .iter()
                    .map(|self_type| format!("{}{}", self_type.name, arguments(&self_type.args)))
                    .collect();
                format!(" : {}", self_types.join(", "))
            };
            writeln!(
                output,
                "{indent}{}module {}{}{self_types}",
                annotations(&module.annotations),
                module.name,
                type_parameters(&module.type_parameters)
            )
            .unwrap();
            dump_members(output, &module.members, depth + 1);
            writeln!(output, "{indent}end").unwrap();
        }
        Declaration::Interface(interface) => {
            writeln!(
                output,
                "{indent}{}interface {}{}",
                annotations(&interface.annotations),
                interface.name,
                type_parameters(&interface.type_parameters)
            )
            .unwrap();
            dump_members(output, &interface.members, depth + 1);
            writeln!(output, "{indent}end").unwrap();
        }
        Declaration::TypeAlias(alias) => {
            writeln!(
                output,
                "{indent}{}type {}{} = {}",
                annotations(&alias.annotations),
                alias.name,
                type_parameters(&alias.type_parameters),
                alias.type_
            )
            .unwrap();
        }
        Declaration::Constant(constant) => {
            writeln!(output, "{indent}{}: {}", constant.name, constant.type_).unwrap();
        }
        Declaration::Global(global) => {
            writeln!(output, "{indent}{}: {}", global.name, global.type_).unwrap();
        }
        Declaration::ClassAlias(alias) => {
            let keyword = match alias.kind {
                ClassOrModule::Class => "class",
                ClassOrModule::Module => "module",
            };
            writeln!(
                output,
                "{indent}{keyword} {} = {}",
                alias.new_name, alias.old_name
            )
            .unwrap();
        }
    }
}

fn dump_members(output: &mut String, members: &[Member], depth: usize) {
    let indent = "  ".repeat(depth);
    for member in members {
        match member {
            Member::MethodDefinition(definition) => {
                let prefix = match definition.kind {
                    MethodKind::Instance => "",
                    MethodKind::Singleton => "self.",
                    MethodKind::SingletonInstance => "self?.",
                };
                let mut overloads: Vec<String> = definition
                    .overloads
                    .iter()
                    .map(|overload| {
                        format!(
                            "{}{}",
                            annotations(&overload.annotations),
                            overload.method_type
                        )
                    })
                    .collect();
                if definition.overloading {
                    overloads.push("...".to_string());
                }
                writeln!(
                    output,
                    "{indent}{}{}def {prefix}{}: {}",
                    annotations(&definition.annotations),
                    visibility(definition.visibility),
                    definition.name,
                    overloads.join(" | ")
                )
                .unwrap();
            }
            Member::Attribute(attribute) => {
                let keyword = match attribute.attribute_kind {
                    AttributeKind::Reader => "attr_reader",
                    AttributeKind::Writer => "attr_writer",
                    AttributeKind::Accessor => "attr_accessor",
                };
                let prefix = match attribute.kind {
                    MemberKind::Instance => "",
                    MemberKind::Singleton => "self.",
                };
                writeln!(
                    output,
                    "{indent}{}{}{keyword} {prefix}{}: {}",
                    annotations(&attribute.annotations),
                    visibility(attribute.visibility),
                    attribute.name,
                    attribute.type_
                )
                .unwrap();
            }
            Member::Alias(alias) => {
                let prefix = match alias.kind {
                    MemberKind::Instance => "",
                    MemberKind::Singleton => "self.",
                };
                writeln!(
                    output,
                    "{indent}{}alias {prefix}{} {prefix}{}",
                    annotations(&alias.annotations),
                    alias.new_name,
                    alias.old_name
                )
                .unwrap();
            }
            Member::Mixin(mixin) => {
                let keyword = match mixin.mixin_kind {
                    MixinKind::Include => "include",
                    MixinKind::Extend => "extend",
                    MixinKind::Prepend => "prepend",
                };
                writeln!(
                    output,
                    "{indent}{keyword} {}{}",
                    mixin.name,
                    arguments(&mixin.args)
                )
                .unwrap();
            }
            Member::Variable(variable) => {
                let prefix = match variable.variable_kind {
                    VariableKind::ClassInstanceVariable => "self.",
                    VariableKind::InstanceVariable | VariableKind::ClassVariable => "",
                };
                writeln!(
                    output,
                    "{indent}{prefix}{}: {}",
                    variable.name, variable.type_
                )
                .unwrap();
            }
            Member::Visibility(Visibility::Public) => writeln!(output, "{indent}public").unwrap(),
            Member::Visibility(Visibility::Private) => writeln!(output, "{indent}private").unwrap(),
            Member::Declaration(declaration) => dump_declaration(output, declaration, depth),
        }
    }
}
