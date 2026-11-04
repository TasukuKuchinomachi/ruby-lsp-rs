use std::{cell::RefCell, rc::Rc};

use rbs_definition::collections::HashMap;
use steep_interface::types::Type;

use crate::{
    diagnostic::ruby::Diagnostic,
    index::source_index::SourceIndex,
    ruby::node::{Node, NodeId, SourceRange},
    ruby_exception::{self, RubyException},
    strict::Observation,
    type_inference::{context::Context, method_call::MethodCall, type_env::TypeEnv},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorRange {
    pub begin: usize,
    pub end: usize,
}

impl CursorRange {
    pub fn new(begin: usize, end: usize) -> CursorRange {
        CursorRange { begin, end }
    }

    fn includes(&self, index: usize) -> bool {
        self.begin <= index && index <= self.end
    }
}

#[derive(Clone, Default)]
pub struct CursorContext {
    pub index: Option<usize>,
    pub data: Option<(CursorRange, Context)>,
}

impl CursorContext {
    pub fn new(index: Option<usize>) -> CursorContext {
        CursorContext { index, data: None }
    }

    pub fn set(&mut self, range: CursorRange, context: &Context) {
        let Some(index) = self.index else {
            return;
        };
        match &self.data {
            Some((current_range, _)) => {
                if range.includes(index)
                    && current_range.begin <= range.begin
                    && range.end <= current_range.end
                {
                    self.data = Some((range, context.clone()));
                }
            }
            None => self.data = Some((range, context.clone())),
        }
    }

    pub fn set_cursor_context(&mut self, other: &CursorContext) {
        if let Some((range, context)) = &other.data {
            self.set(*range, context);
        }
    }

    pub fn set_node_context(&mut self, node: &Node, context: &Context) {
        let expression = node.expression();
        self.set(
            CursorRange::new(expression.begin_pos, expression.end_pos),
            context,
        );
    }

    pub fn set_body_context(&mut self, node: &Node, context: &Context) {
        let end_begin = || {
            node.location
                .end
                .expect("the node has the end keyword")
                .begin_pos
        };
        match node.type_ {
            "class" => {
                let name = node.child_node(0).expect("a class has a name");
                let begin_pos = match node.child_node(1) {
                    Some(super_node) => super_node.expression().end_pos,
                    None => name.expression().end_pos,
                };
                self.set(CursorRange::new(begin_pos, end_begin()), context);
            }
            "module" | "sclass" => {
                let name = node.child_node(0).expect("the node has a name");
                self.set(
                    CursorRange::new(name.expression().end_pos, end_begin()),
                    context,
                );
            }
            "def" | "defs" => {
                if node.children.last().is_some_and(|child| !child.is_nil()) {
                    let args = if node.type_ == "def" {
                        node.child_node(1)
                    } else {
                        node.child_node(2)
                    };
                    let body_begin_pos = match (
                        node.location.assignment,
                        args.and_then(|args| args.location.expression),
                    ) {
                        (Some(assignment), _) => assignment.end_pos,
                        (None, Some(args)) => args.end_pos,
                        (None, None) => {
                            node.location
                                .name
                                .expect("a method definition has a name")
                                .end_pos
                        }
                    };
                    let body_end_pos = match node.location.end {
                        Some(end) => end.begin_pos,
                        None => node.expression().end_pos,
                    };
                    self.set(CursorRange::new(body_begin_pos, body_end_pos), context);
                }
            }
            "block" | "numblock" | "itblock" => {
                self.set(block_range(node), context);
            }
            "for" => {
                let collection = node.child_node(1).expect("a for loop has a collection");
                self.set(
                    CursorRange::new(collection.expression().end_pos, end_begin()),
                    context,
                );
            }
            _ => ruby_exception::raise_runtime_error(format!(
                "Unexpected node for insert_context: {}",
                node.type_
            )),
        }
    }

    pub fn range(&self) -> Option<CursorRange> {
        self.data.as_ref().map(|(range, _)| *range)
    }

    pub fn context(&self) -> Option<&Context> {
        self.data.as_ref().map(|(_, context)| context)
    }
}

pub fn block_range(node: &Node) -> CursorRange {
    let begin_end_pos = || {
        node.location
            .begin
            .expect("a block has the begin keyword")
            .end_pos
    };
    let end_pos = node
        .location
        .end
        .expect("a block has the end keyword")
        .begin_pos;
    let begin_pos = match node.type_ {
        "block" => {
            let send = node.child_node(0).expect("a block has a call");
            match node.child_node(1).and_then(|args| args.location.expression) {
                Some(args) if send.type_ != "lambda" => args.end_pos,
                _ => begin_end_pos(),
            }
        }
        _ => begin_end_pos(),
    };
    CursorRange::new(begin_pos, end_pos)
}

#[derive(Default)]
struct TypingData {
    errors: Vec<Diagnostic>,
    observations: Vec<Observation>,
    typing: Vec<(NodeId, Type)>,
    index: HashMap<NodeId, usize>,
    logic_envs: HashMap<NodeId, (TypeEnv, TypeEnv)>,
    method_calls: HashMap<NodeId, MethodCall>,
    last_update: usize,
    cursor_context: CursorContext,
    root_context: Option<(CursorRange, Context)>,
    source_index: SourceIndex,
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

    pub fn with_cursor(
        cursor: Option<usize>,
        root_context: Option<(CursorRange, Context)>,
    ) -> Typing {
        let mut cursor_context = CursorContext::new(cursor);
        if let Some((range, context)) = &root_context {
            cursor_context.set(*range, context);
        }
        Typing {
            data: Rc::new(RefCell::new(TypingData {
                cursor_context,
                root_context,
                ..TypingData::default()
            })),
            parent: None,
            parent_last_update: 0,
        }
    }

    pub fn cursor_context(&self) -> CursorContext {
        self.data.borrow().cursor_context.clone()
    }

    pub fn has_cursor(&self) -> bool {
        self.data.borrow().cursor_context.index.is_some()
    }

    pub fn update_cursor_context(&self, update: impl FnOnce(&mut CursorContext)) {
        let mut data = self.data.borrow_mut();
        if data.cursor_context.index.is_some() {
            update(&mut data.cursor_context);
        }
    }

    pub fn source_index(&self) -> std::cell::Ref<'_, SourceIndex> {
        std::cell::Ref::map(self.data.borrow(), |data| &data.source_index)
    }

    pub fn take_source_index(&self) -> SourceIndex {
        std::mem::take(&mut self.data.borrow_mut().source_index)
    }

    pub fn update_source_index(&self, update: impl FnOnce(&mut SourceIndex)) {
        update(&mut self.data.borrow_mut().source_index);
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
        let (cursor, root_context) = {
            let data = self.data.borrow();
            (data.cursor_context.index, data.root_context.clone())
        };
        let mut cursor_context = CursorContext::new(cursor);
        if let Some((range, context)) = &root_context {
            cursor_context.set(*range, context);
        }
        Typing {
            data: Rc::new(RefCell::new(TypingData {
                last_update: parent_last_update,
                cursor_context,
                root_context,
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
        let mut parent_data = parent.data.borrow_mut();
        parent_data
            .cursor_context
            .set_cursor_context(&data.cursor_context);
        parent_data.source_index.merge(data.source_index.clone());
    }
}
