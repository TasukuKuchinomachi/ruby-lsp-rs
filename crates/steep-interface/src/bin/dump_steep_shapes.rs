use std::{env, fmt::Write, path::PathBuf, process::ExitCode, sync::Arc};

use rbs_definition::{ast::TypeName, collections::HashMap, environment::Environment, loader::load};
use steep_interface::{
    builder::{Builder, Config},
    steep_version::SteepVersion,
    types::Type,
};

fn config(variable_bounds: HashMap<String, Type>) -> Config {
    let object = TypeName::parse("::Object");
    Config {
        self_type: Some(Type::NameInstance {
            name: object.clone(),
            args: Vec::new(),
        }),
        class_type: Some(Type::Singleton(object.clone())),
        instance_type: Some(Type::NameInstance {
            name: object,
            args: Vec::new(),
        }),
        variable_bounds,
    }
}

fn dump(output: &mut String, builder: &Builder, header: &str, type_: &Type, config: &Config) {
    let entries = match builder
        .shape(type_, config)
        .and_then(|shape| shape.map(|shape| shape.methods(builder)).transpose())
    {
        Ok(Some(entries)) => entries,
        Ok(None) => {
            writeln!(output, "{header}: no shape\n").unwrap();
            return;
        }
        Err(error) => {
            writeln!(output, "{header}: error: {error}\n").unwrap();
            return;
        }
    };
    writeln!(output, "{header}:").unwrap();
    for entry in entries {
        let visibility = if entry.private_method {
            "private"
        } else {
            "public"
        };
        let method_types: Vec<String> = entry
            .method_types()
            .iter()
            .map(ToString::to_string)
            .collect();
        writeln!(
            output,
            "  {visibility} {}: {}",
            entry.method_name,
            method_types.join(" | ")
        )
        .unwrap();
    }
    writeln!(output).unwrap();
}

fn dump_all(
    environment: &Arc<Environment>,
    implicitly_returns_nil: bool,
    steep_version: SteepVersion,
) -> String {
    let mut builder = Builder::new(environment.clone());
    builder.implicitly_returns_nil = implicitly_returns_nil;
    builder.steep_version = steep_version;
    let mut output = String::new();

    let mut class_names: Vec<&TypeName> = environment.class_declarations.keys().collect();
    class_names.sort_by_key(|name| name.to_string());
    for name in class_names {
        let args = environment.class_declarations[name]
            .type_parameter_names()
            .into_iter()
            .map(Type::Var)
            .collect();
        for type_ in [
            Type::NameInstance {
                name: name.clone(),
                args,
            },
            Type::Singleton(name.clone()),
        ] {
            dump(
                &mut output,
                &builder,
                &type_.to_string(),
                &type_,
                &config(HashMap::default()),
            );
        }
    }

    let mut alias_names: Vec<&TypeName> = environment
        .type_alias_declarations
        .keys()
        .filter(|name| name.to_string().starts_with("::ShapeFixture::"))
        .collect();
    alias_names.sort_by_key(|name| name.to_string());
    for name in alias_names {
        let declaration = &environment.type_alias_declarations[name].declaration;
        let variable_bounds = declaration
            .type_parameters
            .iter()
            .filter_map(|parameter| {
                parameter
                    .upper_bound
                    .as_ref()
                    .map(|bound| (parameter.name.clone(), builder.factory.type_(bound)))
            })
            .collect();
        let type_ = builder.factory.type_(&declaration.type_);
        dump(
            &mut output,
            &builder,
            &format!("type {name} = {type_}"),
            &type_,
            &config(variable_bounds),
        );
    }

    for type_ in [Type::SelfType, Type::Instance, Type::Class] {
        dump(
            &mut output,
            &builder,
            &type_.to_string(),
            &type_,
            &config(HashMap::default()),
        );
    }
    output
}

fn main() -> ExitCode {
    let mut arguments: Vec<String> = env::args().skip(1).collect();
    let implicitly_returns_nil = match arguments
        .iter()
        .position(|argument| argument == "--implicitly-returns-nil")
    {
        Some(index) => {
            arguments.remove(index);
            true
        }
        None => false,
    };
    let mut steep_version = SteepVersion::default();
    if let Some(index) = arguments
        .iter()
        .position(|argument| argument.starts_with("--steep-version="))
    {
        let argument = arguments.remove(index);
        let name = &argument["--steep-version=".len()..];
        let Some(version) = SteepVersion::parse(name) else {
            eprintln!(
                "unsupported Steep version {name}: choose one of {}",
                SteepVersion::supported_names()
            );
            return ExitCode::FAILURE;
        };
        steep_version = version;
    }
    let Some(directory) = arguments.first().cloned() else {
        eprintln!(
            "usage: dump_steep_shapes [--implicitly-returns-nil] [--steep-version=VERSION] <directory of the rbs gem> [signature directory...]"
        );
        return ExitCode::FAILURE;
    };
    let directories: Vec<PathBuf> = arguments.iter().skip(1).map(PathBuf::from).collect();
    match load(&PathBuf::from(&directory), &directories) {
        Ok(environment) => {
            print!(
                "{}",
                dump_all(
                    &Arc::new(environment),
                    implicitly_returns_nil,
                    steep_version
                )
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
