use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Instant;

use rbs_environment::{Environment, Loader, MethodKind, Visibility};

fn rbs_gem_directory() -> Option<PathBuf> {
    if let Ok(directory) = std::env::var("RBS_GEM_DIRECTORY") {
        return Some(PathBuf::from(directory));
    }
    let output = Command::new("ruby")
        .args(["-e", "print Gem::Specification.find_by_name('rbs').gem_dir"])
        .output()
        .ok()?;
    let directory = PathBuf::from(String::from_utf8(output.stdout).ok()?);
    (output.status.success() && directory.join("core").is_dir()).then_some(directory)
}

fn core_environment() -> Option<&'static Environment> {
    static ENVIRONMENT: OnceLock<Option<Environment>> = OnceLock::new();
    ENVIRONMENT
        .get_or_init(|| {
            let core_directory = rbs_gem_directory()?.join("core");
            let started_at = Instant::now();
            let mut loader = Loader::new();
            loader.add_library_directory(&core_directory);
            let environment = loader.finish();
            eprintln!(
                "loaded {} classes from core in {:?}",
                environment.classes.len(),
                started_at.elapsed()
            );
            Some(environment)
        })
        .as_ref()
}

macro_rules! core_environment_or_skip {
    () => {
        match core_environment() {
            Some(environment) => environment,
            None => {
                eprintln!("skipped: the rbs gem is not found");
                return;
            }
        }
    };
}

fn instance_overloads(
    environment: &Environment,
    class_name: &str,
    method_name: &str,
) -> Vec<String> {
    let method = environment
        .find_instance_method(class_name, method_name)
        .unwrap_or_else(|| panic!("{class_name}#{method_name} is not found"));
    method.overloads.iter().map(ToString::to_string).collect()
}

fn singleton_overloads(
    environment: &Environment,
    class_name: &str,
    method_name: &str,
) -> Vec<String> {
    let method = environment
        .find_singleton_method(class_name, method_name)
        .unwrap_or_else(|| panic!("{class_name}.{method_name} is not found"));
    method.overloads.iter().map(ToString::to_string).collect()
}

#[test]
fn loads_core_without_errors() {
    let environment = core_environment_or_skip!();
    assert_eq!(environment.errors, Vec::new());
    assert!(environment.classes.len() > 100);
}

#[test]
fn finds_array_map_with_block_returning_array_of_block_result() {
    let environment = core_environment_or_skip!();
    let element = &environment.classes["Array"].type_parameters[0].name;
    let map = environment.find_instance_method("Array", "map").unwrap();
    let with_block = map
        .overloads
        .iter()
        .find(|overload| overload.block.is_some())
        .unwrap();
    let [block_result] = &with_block.type_parameters[..] else {
        panic!("{with_block}")
    };
    let expected = format!(
        "[{0}] () {{ ({element} item) -> {0} }} -> Array[{0}]",
        block_result.name
    );
    assert_eq!(with_block.to_string(), expected);
}

#[test]
fn finds_enumerable_methods_on_array_with_substituted_element_type() {
    let environment = core_environment_or_skip!();
    let ancestors: Vec<String> = environment
        .ancestors("Array")
        .iter()
        .map(|ancestor| ancestor.name.clone())
        .collect();
    assert_eq!(ancestors.first().map(String::as_str), Some("Array"));
    assert!(ancestors.ends_with(&[
        "Object".to_string(),
        "Kernel".to_string(),
        "BasicObject".to_string()
    ]));
    let each_slice = environment
        .find_instance_method("Array", "each_slice")
        .unwrap();
    assert_eq!(each_slice.definition.owner, "Enumerable");
    let element = &environment.classes["Array"].type_parameters[0].name;
    let overloads: Vec<String> = each_slice
        .overloads
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        overloads
            .iter()
            .any(|overload| overload.contains(&format!("Array[{element}]"))),
        "{overloads:?}"
    );
}

#[test]
fn finds_integer_plus() {
    let environment = core_environment_or_skip!();
    let overloads = instance_overloads(environment, "Integer", "+");
    assert!(
        overloads.contains(&"(Integer) -> Integer".to_string()),
        "{overloads:?}"
    );
}

#[test]
fn finds_string_new() {
    let environment = core_environment_or_skip!();
    let overloads = singleton_overloads(environment, "String", "new");
    assert!(!overloads.is_empty());
    assert!(
        overloads
            .iter()
            .all(|overload| overload.ends_with("-> instance")),
        "{overloads:?}"
    );
}

#[test]
fn finds_kernel_puts_through_object() {
    let environment = core_environment_or_skip!();
    let puts = environment.find_instance_method("String", "puts").unwrap();
    assert_eq!(puts.definition.owner, "Kernel");
    assert_eq!(puts.definition.visibility, Visibility::Private);
    assert_eq!(puts.definition.kind, MethodKind::Instance);
    assert!(
        environment
            .find_singleton_method("Kernel", "puts")
            .is_some()
    );
}

#[test]
fn finds_hash_methods_with_pairs_from_enumerable() {
    let environment = core_environment_or_skip!();
    let overloads = instance_overloads(environment, "Hash", "each_with_index");
    assert!(
        overloads.iter().any(|overload| overload.contains("[K, V]")),
        "{overloads:?}"
    );
}

#[test]
fn finds_singleton_methods_of_class_and_module() {
    let environment = core_environment_or_skip!();
    assert!(
        environment
            .find_singleton_method("String", "instance_methods")
            .is_some()
    );
    assert!(
        environment
            .find_singleton_method("Comparable", "instance_methods")
            .is_some()
    );
    assert!(
        environment
            .find_singleton_method("Integer", "sqrt")
            .is_some()
    );
}

#[test]
fn resolves_core_type_aliases_and_constants() {
    let environment = core_environment_or_skip!();
    assert!(environment.expand_alias("int", &[]).is_some());
    assert_eq!(
        environment.resolve_constant_name(&[], "File::SEPARATOR"),
        Some("File::SEPARATOR".to_string())
    );
    assert!(environment.globals.contains_key("$stdout"));
}
