pub mod rbs;
pub mod ruby;
pub mod type_name;

use rbs_definition::{
    ast::{Declaration, MethodType as RbsMethodType, TypeName},
    definition_builder::DefinitionMember,
};
use steep_interface::types::Type;

use crate::{
    method_name::MethodName, ruby::node::SourceRange,
    type_inference::method_call::MethodDeclaration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstantKind {
    Class,
    Module,
    Constant,
}

#[derive(Clone, Debug)]
pub enum CompletionItem {
    InstanceVariable {
        identifier: String,
        range: SourceRange,
        type_: Type,
    },
    KeywordArgument {
        identifier: String,
        range: SourceRange,
    },
    LocalVariable {
        identifier: String,
        range: SourceRange,
        type_: Type,
    },
    Constant {
        identifier: String,
        range: SourceRange,
        type_: Type,
        full_name: TypeName,
        kind: ConstantKind,
        declaration: Option<Declaration>,
        comments: Vec<String>,
        deprecated: bool,
    },
    SimpleMethodName {
        identifier: String,
        range: SourceRange,
        receiver_type: Type,
        method_types: Vec<RbsMethodType>,
        method_member: DefinitionMember,
        method_name: MethodName,
        deprecated: bool,
    },
    ComplexMethodName {
        identifier: String,
        range: SourceRange,
        receiver_type: Type,
        method_types: Vec<RbsMethodType>,
        method_decls: Vec<MethodDeclaration>,
    },
    GeneratedMethodName {
        identifier: String,
        range: SourceRange,
        receiver_type: Type,
        method_types: Vec<RbsMethodType>,
    },
    TypeName {
        absolute_type_name: TypeName,
        relative_type_name: TypeName,
        range: SourceRange,
    },
    Text {
        text: String,
        help_text: String,
        range: SourceRange,
        label: String,
    },
}

impl CompletionItem {
    pub fn range(&self) -> SourceRange {
        match self {
            CompletionItem::InstanceVariable { range, .. }
            | CompletionItem::KeywordArgument { range, .. }
            | CompletionItem::LocalVariable { range, .. }
            | CompletionItem::Constant { range, .. }
            | CompletionItem::SimpleMethodName { range, .. }
            | CompletionItem::ComplexMethodName { range, .. }
            | CompletionItem::GeneratedMethodName { range, .. }
            | CompletionItem::TypeName { range, .. }
            | CompletionItem::Text { range, .. } => *range,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionError {
    SyntaxError,
    UnknownNode,
    Runtime(String),
}
