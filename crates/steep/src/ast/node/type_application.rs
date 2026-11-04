use std::{
    cell::RefCell,
    rc::{Rc, Weak},
    sync::LazyLock,
};

use rbs_definition::ast::TypeName;
use regex::Regex;
use steep_interface::types::Type;

use crate::{
    ast::{
        located_type::{self, ParsingError},
        node::type_assertion::resolve_type_names,
    },
    ruby::node::{Node, SourceRange},
    signature::diagnostic::Diagnostic,
    signature::validator::Validator,
    type_construction::Checker,
};

static APPLICATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A\$\s*(.+)").unwrap());

#[derive(Clone, Debug, PartialEq)]
pub struct LocatedValue<Value> {
    pub value: Value,
    pub location: SourceRange,
}

#[derive(Clone, Debug)]
pub struct TypeApplication {
    pub location: SourceRange,
    pub source: String,
    pub line: usize,
    node: RefCell<Weak<Node>>,
}

impl PartialEq for TypeApplication {
    fn eq(&self, other: &TypeApplication) -> bool {
        self.location == other.location && self.source == other.source
    }
}

pub enum AppliedTypes {
    Types(Vec<LocatedValue<Type>>),
    Errors(Vec<Diagnostic>),
    ParsingError(ParsingError),
}

impl TypeApplication {
    pub fn types(&self, context: &[TypeName], checker: &Checker) -> AppliedTypes {
        let rbs_types =
            match located_type::parse_types(self.type_str(), self.type_location().begin_pos) {
                Ok(types) => types,
                Err(error) => return AppliedTypes::ParsingError(error),
            };
        let mut types = Vec::new();
        for rbs_type in rbs_types {
            let rbs_type = resolve_type_names(rbs_type, context, checker);
            let validator = Validator::new(&checker.builder);
            validator
                .rescue_validation_errors(|validator| validator.validate_type(&rbs_type.type_));
            if validator.has_error() {
                return AppliedTypes::Errors(validator.into_errors());
            }
            types.push(LocatedValue {
                value: checker.builder.factory.type_(&rbs_type.type_),
                location: rbs_type.location,
            });
        }
        AppliedTypes::Types(types)
    }

    pub fn types_or_nothing(
        &self,
        context: &[TypeName],
        checker: &Checker,
    ) -> Option<Vec<LocatedValue<Type>>> {
        match self.types(context, checker) {
            AppliedTypes::Types(types) => Some(types),
            _ => None,
        }
    }

    pub fn type_str(&self) -> &str {
        self.source
            .strip_prefix('$')
            .unwrap_or(&self.source)
            .trim_start()
    }

    pub fn type_location(&self) -> SourceRange {
        let offset = self.source.len() - self.type_str().len();
        SourceRange::new(self.location.begin_pos + offset, self.location.end_pos)
    }

    pub fn parse(source: &str, location: SourceRange, line: usize) -> Option<TypeApplication> {
        APPLICATION.is_match(source).then(|| TypeApplication {
            location,
            source: source.to_string(),
            line,
            node: RefCell::new(Weak::new()),
        })
    }

    pub fn node(&self) -> Rc<Node> {
        self.node
            .borrow()
            .upgrade()
            .expect("a type application is in a tapp node")
    }

    pub fn set_node(&self, node: &Rc<Node>) {
        *self.node.borrow_mut() = Rc::downgrade(node);
    }
}
