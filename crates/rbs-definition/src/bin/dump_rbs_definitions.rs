use std::{env, fmt::Write, path::PathBuf, process::ExitCode};

use rbs_definition::{
    ast::Visibility,
    definition_builder::{Definition, DefinitionBuilder, DefinitionKind},
    environment::Environment,
    loader::load,
};

fn annotations(annotations: &[String]) -> String {
    format!("[{}]", annotations.join(", "))
}

fn dump_definition(output: &mut String, definition: &Definition) {
    let kind = match definition.kind {
        DefinitionKind::Instance => "instance",
        DefinitionKind::Singleton => "singleton",
        DefinitionKind::Interface => "interface",
    };
    let parameters: Vec<String> = definition
        .type_parameters
        .iter()
        .map(ToString::to_string)
        .collect();
    writeln!(
        output,
        "{} {kind} self={} params=[{}]",
        definition.type_name,
        definition.self_type,
        parameters.join(", ")
    )
    .unwrap();
    let mut names: Vec<&String> = definition.methods.keys().collect();
    names.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    for name in names {
        let method = &definition.methods[name];
        let accessibility = match method.accessibility {
            Visibility::Public => "public",
            Visibility::Private => "private",
        };
        let super_method = method
            .super_method
            .as_ref()
            .map_or("-".to_string(), |method| method.defined_in().to_string());
        let alias_of = method
            .alias_of
            .as_ref()
            .map_or("-".to_string(), |method| method.defined_in().to_string());
        writeln!(
            output,
            "  {accessibility} {name}: super={super_method} alias_of={alias_of} annotations={}",
            annotations(&method.annotations)
        )
        .unwrap();
        for type_definition in &method.defs {
            writeln!(
                output,
                "    {} defined_in={} implemented_in={} member={} overload={}",
                type_definition.type_,
                type_definition.defined_in,
                type_definition
                    .implemented_in
                    .as_ref()
                    .map_or("-".to_string(), ToString::to_string),
                annotations(&type_definition.member_annotations),
                annotations(&type_definition.overload_annotations)
            )
            .unwrap();
        }
    }
    for variables in [&definition.instance_variables, &definition.class_variables] {
        let mut names: Vec<&String> = variables.keys().collect();
        names.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        for name in names {
            let variable = &variables[name];
            writeln!(
                output,
                "  {name}: {} declared_in={}",
                variable.type_, variable.declared_in
            )
            .unwrap();
        }
    }
}

fn dump(environment: &Environment) -> Result<String, String> {
    let builder = DefinitionBuilder::new(environment);
    let mut output = String::new();
    let mut class_names: Vec<_> = environment.class_declarations.keys().collect();
    class_names.sort_by_key(|name| name.to_string());
    for name in class_names {
        dump_definition(&mut output, &*builder.build_instance(name)?);
        dump_definition(&mut output, &*builder.build_singleton(name)?);
    }
    let mut interface_names: Vec<_> = environment.interface_declarations.keys().collect();
    interface_names.sort_by_key(|name| name.to_string());
    for name in interface_names {
        dump_definition(&mut output, &*builder.build_interface(name)?);
    }
    Ok(output)
}

fn main() -> ExitCode {
    let Some(directory) = env::args().nth(1) else {
        eprintln!(
            "usage: dump_rbs_definitions <directory of the rbs gem> [signature directory...]"
        );
        return ExitCode::FAILURE;
    };
    let directories: Vec<PathBuf> = env::args().skip(2).map(PathBuf::from).collect();
    match load(&PathBuf::from(&directory), &directories).and_then(|environment| dump(&environment))
    {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
