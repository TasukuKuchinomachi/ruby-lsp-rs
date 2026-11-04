use std::{cell::RefCell, rc::Rc};

use rbs_definition::collections::HashMap;
use steep_interface::types::Type;

use crate::{
    diagnostic::ruby::Diagnostic,
    ruby::node::{Node, NodeId, SourceRange},
    ruby_exception::{self, RubyException},
    strict::Observation,
    type_inference::{method_call::MethodCall, type_env::TypeEnv},
};

#[derive(Default)]
struct TypingData {
    errors: Vec<Diagnostic>,
    observations: Vec<Observation>,
    typing: Vec<(NodeId, Type)>,
    index: HashMap<NodeId, usize>,
    logic_envs: HashMap<NodeId, (TypeEnv, TypeEnv)>,
    method_calls: HashMap<NodeId, MethodCall>,
    last_update: usize,
}

#[derive(Clone, Default)]
pub struct Typing {
    data: Rc<RefCell<TypingData>>,
    parent: Option<Box<Typing>>,
    parent_last_update: usize,
}

impl Typing {
    pub fn new() -> Typing {
        Typing::default()
    }

    pub fn same(&self, other: &Typing) -> bool {
        Rc::ptr_eq(&self.data, &other.data)
    }

    pub fn add_error(&self, error: Diagnostic) {
        self.data.borrow_mut().errors.push(error);
    }

    pub fn errors(&self) -> Vec<Diagnostic> {
        self.data.borrow().errors.clone()
    }

    pub fn has_error_at(&self, location: SourceRange) -> bool {
        self.data
            .borrow()
            .errors
            .iter()
            .any(|error| error.location == Some(location))
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.has_error_at(location))
    }

    pub fn observe(&self, observation: Observation) {
        let mut data = self.data.borrow_mut();
        if !data.observations.contains(&observation) {
            data.observations.push(observation);
        }
    }

    pub fn find_observation(
        &self,
        predicate: &dyn Fn(&Observation) -> bool,
    ) -> Option<Observation> {
        let found = self
            .data
            .borrow()
            .observations
            .iter()
            .find(|observation| predicate(observation))
            .cloned();
        found.or_else(|| {
            self.parent
                .as_ref()
                .and_then(|parent| parent.find_observation(predicate))
        })
    }

    pub fn observations(&self) -> Vec<Observation> {
        self.data.borrow().observations.clone()
    }

    pub fn add_typing(&self, node: NodeId, type_: Type) {
        let mut data = self.data.borrow_mut();
        data.last_update += 1;
        match data.index.get(&node).copied() {
            Some(index) => data.typing[index].1 = type_,
            None => {
                let index = data.typing.len();
                data.typing.push((node, type_));
                data.index.insert(node, index);
            }
        }
    }

    pub fn has_type(&self, node: NodeId) -> bool {
        self.data.borrow().index.contains_key(&node)
    }

    pub fn type_of(&self, node: NodeId) -> Option<Type> {
        let data = self.data.borrow();
        if let Some(index) = data.index.get(&node) {
            return Some(data.typing[*index].1.clone());
        }
        drop(data);
        self.parent.as_ref().and_then(|parent| parent.type_of(node))
    }

    pub fn type_of_or_raise(&self, node: &Node) -> Type {
        self.type_of(node.id).unwrap_or_else(|| {
            ruby_exception::raise(RubyException::new(
                "Steep::Typing::UnknownNodeError",
                format!("Unknown node for type: {node}"),
            ))
        })
    }

    pub fn add_logic_env(&self, node: NodeId, truthy: TypeEnv, falsy: TypeEnv) {
        self.data
            .borrow_mut()
            .logic_envs
            .insert(node, (truthy, falsy));
    }

    pub fn logic_env(&self, node: NodeId) -> Option<(TypeEnv, TypeEnv)> {
        let data = self.data.borrow();
        if let Some(envs) = data.logic_envs.get(&node) {
            return Some(envs.clone());
        }
        drop(data);
        self.parent
            .as_ref()
            .and_then(|parent| parent.logic_env(node))
    }

    pub fn add_call(&self, node: NodeId, call: MethodCall) {
        self.data.borrow_mut().method_calls.insert(node, call);
    }

    pub fn call_of(&self, node: NodeId) -> Option<MethodCall> {
        let data = self.data.borrow();
        if let Some(call) = data.method_calls.get(&node) {
            return Some(call.clone());
        }
        drop(data);
        self.parent.as_ref().and_then(|parent| parent.call_of(node))
    }

    pub fn parent(&self) -> Option<Typing> {
        self.parent.as_deref().cloned()
    }

    fn last_update(&self) -> usize {
        self.data.borrow().last_update
    }

    pub fn new_child(&self) -> Typing {
        let parent_last_update = self.last_update();
        Typing {
            data: Rc::new(RefCell::new(TypingData {
                last_update: parent_last_update,
                ..TypingData::default()
            })),
            parent: Some(Box::new(self.clone())),
            parent_last_update,
        }
    }

    pub fn save(&self) {
        let Some(parent) = &self.parent else {
            ruby_exception::raise_runtime_error("Unexpected save!");
        };
        if parent.last_update() != self.parent_last_update {
            ruby_exception::raise_runtime_error(format!(
                "Parent modified since #new_child: parent.last_update={}, parent_last_update={}",
                parent.last_update(),
                self.parent_last_update
            ));
        }
        let data = self.data.borrow();
        for (node, type_) in &data.typing {
            parent.add_typing(*node, type_.clone());
        }
        for error in &data.errors {
            parent.add_error(error.clone());
        }
        for observation in &data.observations {
            parent.observe(observation.clone());
        }
        for (node, (truthy, falsy)) in &data.logic_envs {
            parent.add_logic_env(*node, truthy.clone(), falsy.clone());
        }
        for (node, call) in &data.method_calls {
            parent.add_call(*node, call.clone());
        }
    }
}
