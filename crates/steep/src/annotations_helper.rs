use std::sync::LazyLock;

use rbs_definition::{
    ast::{TypeName, TypeNameKind},
    environment::{ClassOrModuleDeclaration, Environment},
};
use regex::Regex;

static DEPRECATED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"deprecated(:\s*(?<message>.+))?").unwrap());
static STEEP_DEPRECATED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"steep:deprecated(:\s*(?<message>.+))?").unwrap());

pub fn deprecated_annotation(annotations: &[String]) -> Option<(&String, Option<String>)> {
    deprecated_annotation_in(annotations.iter())
}

pub fn deprecated_annotation_in<'annotations>(
    annotations: impl DoubleEndedIterator<Item = &'annotations String>,
) -> Option<(&'annotations String, Option<String>)> {
    for annotation in annotations
        .rev()
        .filter(|annotation| annotation.contains("deprecated"))
    {
        for pattern in [&DEPRECATED, &STEEP_DEPRECATED] {
            if let Some(captures) = pattern.captures(annotation) {
                let message = captures
                    .name("message")
                    .map(|message| message.as_str().to_string());
                return Some((annotation, message));
            }
        }
    }
    None
}

fn class_or_module_annotations(declaration: &ClassOrModuleDeclaration) -> &[String] {
    match declaration {
        ClassOrModuleDeclaration::Class(class) => &class.annotations,
        ClassOrModuleDeclaration::Module(module) => &module.annotations,
    }
}

pub fn class_entry_annotations<'environment>(
    environment: &'environment Environment,
    type_name: &TypeName,
) -> Option<impl DoubleEndedIterator<Item = &'environment String>> {
    environment.class_declarations.get(type_name).map(|entry| {
        entry
            .declarations()
            .flat_map(|declaration| class_or_module_annotations(declaration).iter())
    })
}

pub fn deprecated_type_name(
    type_name: &TypeName,
    environment: &Environment,
) -> Option<Option<String>> {
    let found = match type_name.kind() {
        TypeNameKind::Class => match class_entry_annotations(environment, type_name) {
            Some(annotations) => deprecated_annotation_in(annotations),
            None => deprecated_annotation(
                &environment
                    .class_alias_declarations
                    .get(type_name)?
                    .declaration
                    .annotations,
            ),
        },
        TypeNameKind::Interface => deprecated_annotation(
            &environment
                .interface_declarations
                .get(type_name)?
                .declaration
                .annotations,
        ),
        TypeNameKind::Alias => deprecated_annotation(
            &environment
                .type_alias_declarations
                .get(type_name)?
                .declaration
                .annotations,
        ),
    };
    found.map(|(_, message)| message)
}
