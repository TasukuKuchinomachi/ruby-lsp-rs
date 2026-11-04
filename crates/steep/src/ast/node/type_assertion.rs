use std::sync::LazyLock;

use rbs_definition::ast::TypeName;
use regex::Regex;
use steep_interface::types::Type;

use crate::{
    ast::located_type::{self, LocatedType, ParsingError},
    ruby::node::SourceRange,
    signature::diagnostic::Diagnostic,
    signature::validator::Validator,
    type_construction::Checker,
};

static ASSERTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A:\s*(.+)").unwrap());
static RDOC_DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A:[a-z][a-z-]*:").unwrap());

#[derive(Clone, Debug, PartialEq)]
pub struct TypeAssertion {
    pub location: SourceRange,
    pub source: String,
    pub line: usize,
}

pub enum AssertedType {
    Type(Type),
    Errors(Vec<Diagnostic>),
    Nothing,
    ParsingError(ParsingError),
}

pub fn resolve_type_names(
    type_: LocatedType,
    context: &[TypeName],
    checker: &Checker,
) -> LocatedType {
    type_.map_type_name(&mut |name| {
        checker
            .type_name_resolver
            .resolve(name, context)
            .unwrap_or_else(|| name.absolute())
    })
}

impl TypeAssertion {
    pub fn type_str(&self) -> &str {
        self.source
            .strip_prefix(':')
            .unwrap_or(&self.source)
            .trim_start()
    }

    pub fn type_location(&self) -> SourceRange {
        let offset = self.source.len() - self.type_str().len();
        SourceRange::new(self.location.begin_pos + offset, self.location.end_pos)
    }

    pub fn rbs_type(
        &self,
        context: &[TypeName],
        checker: &Checker,
    ) -> Result<Option<LocatedType>, ParsingError> {
        let type_ = located_type::parse_type(self.type_str(), self.type_location().begin_pos)?;
        Ok(type_.map(|type_| resolve_type_names(type_, context, checker)))
    }

    pub fn type_(&self, context: &[TypeName], checker: &Checker) -> AssertedType {
        match self.rbs_type(context, checker) {
            Ok(Some(type_)) => {
                let validator = Validator::new(&checker.builder);
                validator
                    .rescue_validation_errors(|validator| validator.validate_type(&type_.type_));
                if validator.has_error() {
                    AssertedType::Errors(validator.into_errors())
                } else {
                    AssertedType::Type(checker.builder.factory.type_(&type_.type_))
                }
            }
            Ok(None) => AssertedType::Nothing,
            Err(error) => AssertedType::ParsingError(error),
        }
    }

    pub fn type_syntax(&self) -> bool {
        matches!(
            located_type::parse_type(self.type_str(), self.type_location().begin_pos),
            Ok(Some(_))
        )
    }

    pub fn type_or_nothing(&self, context: &[TypeName], checker: &Checker) -> Option<Type> {
        match self.type_(context, checker) {
            AssertedType::Type(type_) => Some(type_),
            _ => None,
        }
    }

    pub fn looks_like_assertion(source: &str) -> bool {
        let source = source.trim();
        ASSERTION.is_match(source) && !RDOC_DIRECTIVE.is_match(source)
    }

    pub fn parse(source: &str, location: SourceRange, line: usize) -> Option<TypeAssertion> {
        let source = source.trim();
        if ASSERTION.is_match(source) {
            let assertion = TypeAssertion {
                location,
                source: source.to_string(),
                line,
            };
            if assertion.type_syntax() {
                return Some(assertion);
            }
        }
        None
    }
}
