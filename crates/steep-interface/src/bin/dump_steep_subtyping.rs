use rbs_definition::errors::RbsError;
use std::{env, fmt::Write, path::PathBuf, process::ExitCode, sync::Arc};

use rbs_definition::{ast::TypeName, environment::Environment, loader::load};
use steep_interface::{
    builder::Builder,
    subtyping::{
        check::Check, constraints::Constraints, relation::Relation, result::SubtypingResult,
        variable_variance::VariableVariance,
    },
    types::Type,
};

fn result_line(result: &SubtypingResult) -> String {
    let relation = result.relation();
    let mut line = format!("{} {}: {relation}", result.class_name(), relation.kind());
    if let SubtypingResult::Failure { error, .. } = result {
        write!(line, " ({}: {})", error.class_name(), error.message()).unwrap();
    }
    line
}

fn write_tree(output: &mut String, result: &SubtypingResult, depth: usize) {
    writeln!(output, "{}{}", "  ".repeat(depth), result_line(result)).unwrap();
    match result {
        SubtypingResult::Expand { child, .. } => write_tree(output, child, depth + 1),
        SubtypingResult::All { branches, .. } | SubtypingResult::Any { branches, .. } => {
            for branch in branches {
                write_tree(output, branch, depth + 1);
            }
        }
        _ => {}
    }
}

fn write_result(output: &mut String, result: &SubtypingResult) {
    let status = if result.is_success() {
        "success"
    } else {
        "failure"
    };
    writeln!(output, "  {status}").unwrap();
    writeln!(output, "  tree:").unwrap();
    write_tree(output, result, 2);
    if result.is_success() {
        return;
    }
    let path = result.failure_path();
    writeln!(output, "  failure path:").unwrap();
    for step in &path {
        writeln!(output, "    {}", result_line(step)).unwrap();
    }
    writeln!(output, "  diagnostic:").unwrap();
    for (index, message) in path
        .iter()
        .filter_map(|step| step.relation().diagnostic_message())
        .enumerate()
    {
        writeln!(output, "  {}{message}", "  ".repeat(index + 1)).unwrap();
    }
}

fn dump_relation(
    output: &mut String,
    checker: &Check,
    relation: Relation<Type>,
    variables: &[String],
    variance: &VariableVariance,
    self_type: &Type,
    class_type: &Type,
) -> Result<(), RbsError> {
    let constraints = Constraints::new(variables).shared();
    let result = checker.check(
        relation,
        constraints.clone(),
        Some(self_type.clone()),
        Some(self_type.clone()),
        Some(class_type.clone()),
    )?;
    write_result(output, &result);
    writeln!(output, "  constraints: {}", constraints.borrow()).unwrap();
    if variables.is_empty() {
        return Ok(());
    }
    let solution = constraints.borrow().solution(
        checker,
        variance,
        variables,
        Some(self_type.clone()),
        Some(self_type.clone()),
        Some(class_type.clone()),
    )?;
    match solution {
        Ok(solution) => {
            let pairs: Vec<String> = solution
                .iter()
                .map(|(variable, type_)| format!("{variable} => {type_}"))
                .collect();
            writeln!(output, "  solution: {}", pairs.join(", ")).unwrap();
        }
        Err(error) => writeln!(
            output,
            "  unsatisfiable: {}: {} <: {}",
            error.variable, error.sub_type, error.super_type
        )
        .unwrap(),
    }
    Ok(())
}

fn dump_all(environment: &Arc<Environment>) -> Result<String, RbsError> {
    let builder = Builder::new(environment.clone());
    let checker = Check::new(&builder);
    let mut output = String::new();

    let mut class_names: Vec<&TypeName> = environment
        .class_declarations
        .keys()
        .filter(|name| name.to_string().starts_with("::SubtypingFixture::"))
        .filter(|name| name.name.ends_with("Relations"))
        .collect();
    class_names.sort_by_key(|name| name.to_string());
    for type_name in class_names {
        let class_params: Vec<_> = environment.class_declarations[type_name]
            .type_parameters()
            .iter()
            .map(|parameter| builder.factory.type_param(parameter))
            .collect();
        let self_type = Type::NameInstance {
            name: type_name.clone(),
            args: class_params
                .iter()
                .map(|parameter| Type::Var(parameter.name.clone()))
                .collect(),
        };
        let class_type = Type::Singleton(type_name.clone());
        let definition = builder.definition_builder.build_instance(type_name)?;
        for (method_name, method) in &definition.methods {
            let definition = &method.defs[0];
            if definition.defined_in != *type_name {
                continue;
            }
            let method_type = builder.factory.method_type(&definition.type_);
            let sub_type = method_type
                .type_
                .params
                .as_ref()
                .and_then(|params| params.required_types().first().cloned().cloned())
                .ok_or_else(|| format!("{type_name}#{method_name} has no parameter"))?;
            let relation = Relation::new(sub_type, method_type.type_.return_type.clone());
            let variables: Vec<String> = method_type
                .type_params
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect();
            writeln!(output, "{type_name}#{method_name}: {relation}").unwrap();
            let mut type_params = class_params.clone();
            type_params.extend(method_type.type_params.iter().cloned());
            let dumped = checker.push_type_params(&type_params, || {
                dump_relation(
                    &mut output,
                    &checker,
                    relation,
                    &variables,
                    &VariableVariance::from_method_type(&method_type),
                    &self_type,
                    &class_type,
                )
            });
            if dumped.is_err() {
                writeln!(output, "  error").unwrap();
            }
            writeln!(output).unwrap();
        }
    }
    Ok(output)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let Some(directory) = arguments.first() else {
        eprintln!(
            "usage: dump_steep_subtyping <directory of the rbs gem> [signature directory...]"
        );
        return ExitCode::FAILURE;
    };
    let directories: Vec<PathBuf> = arguments.iter().skip(1).map(PathBuf::from).collect();
    match load(&PathBuf::from(directory), &directories)
        .and_then(|environment| dump_all(&Arc::new(environment)).map_err(|error| error.to_string()))
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
