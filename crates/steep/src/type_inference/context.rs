use std::sync::Arc;
use std::{cell::RefCell, rc::Rc};

use rbs_definition::{
    ast::TypeName,
    collections::{HashMap, IndexMap},
    definition_builder::{Definition, Method},
};
use steep_interface::{
    method_type::{Block, MethodType, Params, TypeParam},
    substitution::Substitution,
    types::Type,
};

use crate::type_inference::{constant_env::ResolverContext, type_env::TypeEnv};

#[derive(Clone, Debug)]
pub enum ForwardArgType {
    Untyped,
    Typed(Box<(Params, Option<Block>)>),
}

#[derive(Clone)]
pub struct MethodContext {
    pub name: Option<String>,
    pub method: Option<Arc<Method>>,
    pub method_type: Option<MethodType>,
    pub return_type: Type,
    pub super_method: Option<Arc<Method>>,
    pub forward_arg_type: Option<ForwardArgType>,
    pub block_param_name: Option<String>,
}

impl MethodContext {
    pub fn block_type(&self) -> Option<&Block> {
        self.method_type
            .as_ref()
            .and_then(|method_type| method_type.block.as_ref())
    }

    pub fn is_attribute_setter(&self) -> bool {
        self.name.as_ref().is_some_and(|name| name.ends_with('='))
    }
}

#[derive(Clone)]
pub struct BlockContext {
    pub body_type: Option<Type>,
}

impl BlockContext {
    pub fn substitute(&self, substitution: &Substitution) -> BlockContext {
        BlockContext {
            body_type: self
                .body_type
                .as_ref()
                .map(|type_| type_.substitute(substitution)),
        }
    }
}

#[derive(Clone)]
pub struct BreakContext {
    pub break_type: Type,
    pub next_type: Option<Type>,
    pub unchecked_break: bool,
    pub unchecked_next: bool,
}

impl BreakContext {
    pub fn substitute(&self, substitution: &Substitution) -> BreakContext {
        BreakContext {
            break_type: self.break_type.substitute(substitution),
            next_type: self
                .next_type
                .as_ref()
                .map(|type_| type_.substitute(substitution)),
            unchecked_break: self.unchecked_break,
            unchecked_next: self.unchecked_next,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImplementName {
    pub name: TypeName,
    pub args: Vec<String>,
}

#[derive(Clone)]
pub struct ModuleContext {
    pub instance_type: Type,
    pub module_type: Type,
    pub implement_name: Option<ImplementName>,
    pub nesting: ResolverContext,
    pub class_name: TypeName,
    pub instance_definition: Option<Arc<Definition>>,
    pub module_definition: Option<Arc<Definition>>,
    pub defined_instance_methods: Rc<RefCell<Vec<String>>>,
    pub defined_module_methods: Rc<RefCell<Vec<String>>>,
}

#[derive(Clone, Default)]
pub struct TypeVariableContext {
    pub table: IndexMap<String, TypeParam>,
}

impl TypeVariableContext {
    pub fn new(
        type_params: &[TypeParam],
        parent: Option<&TypeVariableContext>,
    ) -> TypeVariableContext {
        let mut table = parent
            .map(|parent| parent.table.clone())
            .unwrap_or_default();
        for param in type_params {
            table.insert(param.name.clone(), param.clone());
        }
        TypeVariableContext { table }
    }

    pub fn empty() -> TypeVariableContext {
        TypeVariableContext::default()
    }

    pub fn upper_bounds(&self) -> HashMap<String, Type> {
        self.table
            .values()
            .filter_map(|param| {
                param
                    .upper_bound
                    .clone()
                    .map(|bound| (param.name.clone(), bound))
            })
            .collect()
    }

    pub fn upper_bounds_list(&self) -> Vec<(String, Option<Type>)> {
        self.table
            .values()
            .filter(|param| param.upper_bound.is_some())
            .map(|param| (param.name.clone(), param.upper_bound.clone()))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CallContext {
    TopLevel,
    Unknown,
    Module(TypeName),
    Method {
        type_name: TypeName,
        singleton: bool,
        method_name: String,
    },
}

#[derive(Clone)]
pub struct Context {
    pub method_context: Option<Rc<MethodContext>>,
    pub block_context: Option<BlockContext>,
    pub break_context: Option<BreakContext>,
    pub module_context: Rc<ModuleContext>,
    pub self_type: Type,
    pub type_env: TypeEnv,
    pub call_context: CallContext,
    pub variable_context: Rc<TypeVariableContext>,
}
