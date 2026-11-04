use std::path::{Path, PathBuf};

use rbs_definition::{
    ancestor_builder::AncestorBuilder, ast::TypeName, definition_builder::DefinitionBuilder,
    environment::Environment, loader::load,
};

fn tests_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn environment() -> Environment {
    load(
        &tests_directory().join("minimal_rbs_gem"),
        &[tests_directory().join("signatures")],
    )
    .expect("the signatures load")
}

#[test]
fn builds_instance_ancestors_with_type_arguments_of_mixins() {
    let environment = environment();
    let builder = AncestorBuilder::new(&environment);
    let ancestors = builder
        .instance_ancestors(&TypeName::parse("::StringBox"))
        .expect("the ancestors are built");
    let names: Vec<String> = ancestors
        .ancestors
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        names,
        [
            "::StringBox",
            "::Box[::String]",
            "::Comparing[::String]",
            "::Object",
            "::Kernel",
            "::BasicObject",
        ]
    );
}

#[test]
fn builds_overloading_methods_with_the_super_method() {
    let environment = environment();
    let builder = DefinitionBuilder::new(&environment);
    let definition = builder
        .build_instance(&TypeName::parse("::StringBox"))
        .expect("the definition is built");
    let map = &definition.methods["map"];
    let method_types: Vec<String> = map
        .defs
        .iter()
        .map(|type_definition| type_definition.type_.to_string())
        .collect();
    assert_eq!(
        method_types,
        [
            "[U] () { (::String) -> U } -> ::Box[U]",
            "[U] () { (::String) -> U } -> ::Box[U]",
        ]
    );
    let super_method = map.super_method.as_ref().expect("map has a super method");
    assert_eq!(super_method.defs[0].defined_in.to_string(), "::Box");
    let fmap = &definition.methods["fmap"];
    assert!(fmap.alias_of.is_some());
    assert_eq!(fmap.defs[0].defined_in.to_string(), "::Box");
    assert_eq!(
        definition.instance_variables["@value"].type_.to_string(),
        "::String"
    );
}
