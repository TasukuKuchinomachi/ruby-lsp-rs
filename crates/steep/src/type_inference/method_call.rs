use std::rc::Rc;

use rbs_definition::ast::TypeName;
use steep_interface::{method_type::MethodType, types::Type};

use crate::{diagnostic::ruby::Diagnostic, ruby::node::Node};

#[derive(Clone, Debug, PartialEq)]
pub struct MethodDeclaration {
    pub type_name: TypeName,
    pub singleton: bool,
    pub method_name: String,
    pub annotations: Vec<String>,
    pub attribute: bool,
    pub method_type: rbs_definition::ast::MethodType,
}

impl MethodDeclaration {
    pub fn full_name(&self) -> String {
        let separator = if self.singleton { "." } else { "#" };
        format!("{}{separator}{}", self.type_name, self.method_name)
    }
}

#[derive(Clone, Debug)]
pub enum MethodCallKind {
    Typed { actual_method_type: MethodType },
    Special { actual_method_type: MethodType },
    Untyped,
    NoMethodError { error: Diagnostic },
    Error { errors: Vec<Diagnostic> },
}

#[derive(Clone, Debug)]
pub struct MethodCall {
    pub node: Rc<Node>,
    pub method_name: String,
    pub receiver_type: Type,
    pub return_type: Type,
    pub method_decls: Vec<MethodDeclaration>,
    pub kind: MethodCallKind,
}

pub const KNOWN_PURE_METHODS: &[&str] = &["::Array#[]", "::Hash#[]"];

impl MethodCall {
    pub fn is_typed(&self) -> bool {
        matches!(
            self.kind,
            MethodCallKind::Typed { .. } | MethodCallKind::Special { .. }
        )
    }

    pub fn with_return_type(&self, return_type: Type) -> MethodCall {
        MethodCall {
            return_type,
            ..self.clone()
        }
    }

    pub fn is_pure(&self) -> bool {
        self.method_decls.iter().all(|declaration| {
            if declaration.attribute {
                !declaration.method_name.ends_with('=')
            } else {
                declaration
                    .annotations
                    .iter()
                    .any(|annotation| annotation == "pure")
            }
        })
    }

    pub fn is_known_pure(&self) -> bool {
        self.method_decls
            .iter()
            .all(|declaration| KNOWN_PURE_METHODS.contains(&declaration.full_name().as_str()))
    }

    pub fn same_call(&self, other: &MethodCall) -> bool {
        self.node.id == other.node.id
            && self.method_name == other.method_name
            && self.receiver_type == other.receiver_type
            && self.return_type == other.return_type
            && self.method_decls == other.method_decls
    }
}
