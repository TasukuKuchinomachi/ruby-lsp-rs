use std::fmt;

use rbs_definition::ast::TypeName;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MethodName {
    Instance {
        type_name: TypeName,
        method_name: String,
    },
    Singleton {
        type_name: TypeName,
        method_name: String,
    },
}

impl MethodName {
    pub fn instance(type_name: TypeName, method_name: impl Into<String>) -> MethodName {
        MethodName::Instance {
            type_name,
            method_name: method_name.into(),
        }
    }

    pub fn singleton(type_name: TypeName, method_name: impl Into<String>) -> MethodName {
        MethodName::Singleton {
            type_name,
            method_name: method_name.into(),
        }
    }

    pub fn type_name(&self) -> &TypeName {
        match self {
            MethodName::Instance { type_name, .. } | MethodName::Singleton { type_name, .. } => {
                type_name
            }
        }
    }

    pub fn method_name(&self) -> &str {
        match self {
            MethodName::Instance { method_name, .. }
            | MethodName::Singleton { method_name, .. } => method_name,
        }
    }

    pub fn is_singleton(&self) -> bool {
        matches!(self, MethodName::Singleton { .. })
    }

    pub fn relative(&self) -> MethodName {
        match self {
            MethodName::Instance {
                type_name,
                method_name,
            } => MethodName::instance(type_name.relative(), method_name.clone()),
            MethodName::Singleton {
                type_name,
                method_name,
            } => MethodName::singleton(type_name.relative(), method_name.clone()),
        }
    }
}

impl fmt::Display for MethodName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MethodName::Instance {
                type_name,
                method_name,
            } => write!(formatter, "{type_name}#{method_name}"),
            MethodName::Singleton {
                type_name,
                method_name,
            } => write!(formatter, "{type_name}.{method_name}"),
        }
    }
}
