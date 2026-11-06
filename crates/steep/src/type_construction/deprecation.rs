use rbs_definition::ast::TypeName;

use crate::{
    annotations_helper::{
        class_entry_annotations, deprecated_annotation, deprecated_annotation_in,
    },
    diagnostic::ruby as diagnostics,
    ruby::node::{Node, SourceRange},
    type_construction::TypeConstruction,
};

impl<'checker> TypeConstruction<'checker> {
    pub fn check_deprecation_global(&self, name: &str, node: &Node, location: Option<SourceRange>) {
        if let Some(global_entry) = self.checker.environment().global_declarations.get(name)
            && let Some((_, message)) = deprecated_annotation(&global_entry.annotations)
        {
            self.add_error(diagnostics::deprecated_reference(
                node,
                location,
                message.as_deref(),
            ));
        }
    }

    pub fn check_deprecation_constant(
        &self,
        name: &TypeName,
        node: &Node,
        location: Option<SourceRange>,
    ) {
        let environment = self.checker.environment();
        let found = if let Some(annotations) = class_entry_annotations(environment, name) {
            deprecated_annotation_in(annotations)
        } else if let Some(entry) = environment.constant_declarations.get(name) {
            deprecated_annotation(&entry.declaration.annotations)
        } else if let Some(entry) = environment.class_alias_declarations.get(name) {
            deprecated_annotation(&entry.declaration.annotations)
        } else {
            None
        };
        if let Some((_, message)) = found {
            self.add_error(diagnostics::deprecated_reference(
                node,
                location,
                message.as_deref(),
            ));
        }
    }
}
