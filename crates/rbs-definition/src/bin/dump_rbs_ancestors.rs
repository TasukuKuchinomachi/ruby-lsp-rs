use std::{env, path::PathBuf, process::ExitCode, sync::Arc};

use rbs_definition::{
    ancestor_builder::{Ancestor, AncestorBuilder, Ancestors, OneAncestors},
    environment::Environment,
    loader::load,
};

fn one_ancestors(one_ancestors: &OneAncestors) -> String {
    let super_class = one_ancestors
        .super_class
        .as_ref()
        .map(|super_class| vec![super_class.clone()]);
    let fields: [(&str, Option<&Vec<Ancestor>>); 7] = [
        ("super", super_class.as_ref()),
        ("self_types", one_ancestors.self_types.as_ref()),
        ("include", one_ancestors.included_modules.as_ref()),
        (
            "include_interface",
            one_ancestors.included_interfaces.as_ref(),
        ),
        ("prepend", one_ancestors.prepended_modules.as_ref()),
        ("extend", one_ancestors.extended_modules.as_ref()),
        (
            "extend_interface",
            one_ancestors.extended_interfaces.as_ref(),
        ),
    ];
    fields
        .iter()
        .filter_map(|(name, ancestors)| {
            ancestors.map(|ancestors| {
                let ancestors: Vec<String> = ancestors.iter().map(ToString::to_string).collect();
                format!("{name}=[{}]", ancestors.join(", "))
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn ancestors(ancestors: &Ancestors) -> String {
    ancestors
        .ancestors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" < ")
}

fn dump(environment: Arc<Environment>) -> Result<String, String> {
    let builder = AncestorBuilder::new(environment.clone());
    let mut output = String::new();
    let mut class_names: Vec<_> = environment.class_declarations.keys().collect();
    class_names.sort_by_key(|name| name.to_string());
    for name in class_names {
        output.push_str(&format!("{name}\n"));
        output.push_str(&format!(
            "  one instance: {}\n",
            one_ancestors(&*builder.one_instance_ancestors(name)?)
        ));
        output.push_str(&format!(
            "  one singleton: {}\n",
            one_ancestors(&*builder.one_singleton_ancestors(name)?)
        ));
        output.push_str(&format!(
            "  instance: {}\n",
            ancestors(&*builder.instance_ancestors(name)?)
        ));
        output.push_str(&format!(
            "  singleton: {}\n",
            ancestors(&*builder.singleton_ancestors(name)?)
        ));
    }
    let mut interface_names: Vec<_> = environment.interface_declarations.keys().collect();
    interface_names.sort_by_key(|name| name.to_string());
    for name in interface_names {
        output.push_str(&format!("{name}\n"));
        output.push_str(&format!(
            "  one interface: {}\n",
            one_ancestors(&*builder.one_interface_ancestors(name)?)
        ));
        output.push_str(&format!(
            "  interface: {}\n",
            ancestors(&*builder.interface_ancestors(name)?)
        ));
    }
    Ok(output)
}

fn main() -> ExitCode {
    let Some(directory) = env::args().nth(1) else {
        eprintln!("usage: dump_rbs_ancestors <directory of the rbs gem> [signature directory...]");
        return ExitCode::FAILURE;
    };
    match load(
        &PathBuf::from(&directory),
        &env::args().skip(2).map(PathBuf::from).collect::<Vec<_>>(),
    )
    .and_then(|environment| dump(Arc::new(environment)))
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
