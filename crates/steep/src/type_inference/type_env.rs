use std::rc::Rc;

use rbs_definition::{ast::TypeName, collections::IndexMap};
use steep_interface::{substitution::Substitution, types::Type};

use crate::{
    ruby::node::{Child, Location, MapKind, Node},
    ruby_exception::{self, RubyException},
    type_inference::{constant_env::ConstantEnv, method_call::MethodCall},
};

#[derive(Clone, Debug)]
pub struct PureCall {
    pub node: Rc<Node>,
    pub call: Rc<MethodCall>,
    pub type_: Option<Type>,
}

#[derive(Clone)]
pub struct TypeEnv {
    pub constant_env: Rc<ConstantEnv>,
    pub local_variable_types: IndexMap<String, (Type, Option<Type>)>,
    pub instance_variable_types: Rc<IndexMap<String, Type>>,
    pub global_types: Rc<IndexMap<String, Type>>,
    pub constant_types: IndexMap<TypeName, Type>,
    pub pure_method_calls: IndexMap<String, PureCall>,
}

pub fn node_key(node: &Node) -> String {
    node.to_string()
}

fn descendant_keys(node: &Node, keys: &mut Vec<String>) {
    for child in node.child_nodes() {
        keys.push(node_key(child));
        descendant_keys(child, keys);
    }
}

pub const SPECIAL_LVAR_NAMES: &[&str] = &["_", "__any__", "__skip__"];

fn ensure_local_variable_name(name: &str) {
    if !is_local_variable_name(name) {
        ruby_exception::raise_runtime_error(format!("{name} is not a local variable"));
    }
}

pub fn is_local_variable_name(name: &str) -> bool {
    let Some(first) = name.chars().next() else {
        return true;
    };
    if first.is_uppercase() || first == '@' || first == '$' {
        return false;
    }
    !SPECIAL_LVAR_NAMES.contains(&name)
}

fn lvar_key(name: &str) -> String {
    node_key(&Node {
        id: 0,
        type_: "lvar",
        children: vec![Child::Symbol(name.to_string())],
        location: Location::new(MapKind::Map, None),
    })
}

impl TypeEnv {
    pub fn new(constant_env: Rc<ConstantEnv>) -> TypeEnv {
        TypeEnv {
            constant_env,
            local_variable_types: IndexMap::default(),
            instance_variable_types: Rc::default(),
            global_types: Rc::default(),
            constant_types: IndexMap::default(),
            pure_method_calls: IndexMap::default(),
        }
    }

    pub fn get(&self, name: &str) -> Option<Type> {
        if name.starts_with('$') {
            return self.global_types.get(name).cloned();
        }
        if name.starts_with('@') {
            return self.instance_variable_types.get(name).cloned();
        }
        self.local_variable_types
            .get(name)
            .map(|(type_, _)| type_.clone())
    }

    pub fn get_node(&self, node: &Node) -> Option<Type> {
        match node.type_ {
            "lvar" => node.child_symbol(0).and_then(|name| self.get(name)),
            "send" => self.pure_method_calls.get(&node_key(node)).map(|pure| {
                pure.type_
                    .clone()
                    .unwrap_or_else(|| pure.call.return_type.clone())
            }),
            _ => None,
        }
    }

    pub fn enforced_type(&self, name: &str) -> Option<Type> {
        self.local_variable_types
            .get(name)
            .and_then(|(_, enforced)| enforced.clone())
    }

    fn merge_pure_calls(&mut self, updates: IndexMap<String, PureCall>) {
        for (key, pure) in updates {
            self.pure_method_calls.insert(key, pure);
        }
    }

    pub fn assign_local_variables(&self, assignments: &[(String, Type)]) -> TypeEnv {
        let mut env = self.clone();
        let mut invalidated = Vec::new();
        for (name, type_) in assignments {
            ensure_local_variable_name(name);
            env.local_variable_types
                .insert(name.clone(), (type_.clone(), self.enforced_type(name)));
            invalidated.extend(self.invalidated_pure_nodes(&lvar_key(name)));
        }
        let invalidation = self.pure_node_invalidation(&invalidated);
        env.merge_pure_calls(invalidation);
        env
    }

    pub fn assign_local_variable(
        &self,
        name: &str,
        variable_type: Type,
        enforced_type: Option<Type>,
    ) -> TypeEnv {
        ensure_local_variable_name(name);
        let mut env = self.clone();
        env.local_variable_types.insert(
            name.to_string(),
            (
                enforced_type.clone().unwrap_or(variable_type),
                enforced_type,
            ),
        );
        let invalidation =
            self.pure_node_invalidation(&self.invalidated_pure_nodes(&lvar_key(name)));
        env.merge_pure_calls(invalidation);
        env
    }

    pub fn refine_types(
        &self,
        local_variable_types: &[(String, Type)],
        pure_call_types: &[(String, Type)],
    ) -> TypeEnv {
        let mut env = self.clone();
        for (name, type_) in local_variable_types {
            ensure_local_variable_name(name);
            env.local_variable_types
                .insert(name.clone(), (type_.clone(), self.enforced_type(name)));
        }
        let mut invalidated: Vec<String> =
            pure_call_types.iter().map(|(key, _)| key.clone()).collect();
        for (name, _) in local_variable_types {
            invalidated.extend(self.invalidated_pure_nodes(&lvar_key(name)));
        }
        let mut updates = self.pure_node_invalidation(&invalidated);
        for (key, type_) in pure_call_types {
            let Some(pure) = updates.get_mut(key) else {
                ruby_exception::raise(RubyException::new(
                    "KeyError",
                    format!("key not found: {key}"),
                ));
            };
            pure.type_ = Some(type_.clone());
        }
        env.merge_pure_calls(updates);
        env
    }

    pub fn pin_local_variables(&self, names: Option<&[String]>) -> IndexMap<String, (Type, Type)> {
        let mut pinned = IndexMap::default();
        for (name, (type_, enforced)) in &self.local_variable_types {
            ensure_local_variable_name(name);
            if names.is_none_or(|names| names.contains(name)) && enforced.is_none() {
                pinned.insert(name.clone(), (type_.clone(), type_.clone()));
            }
        }
        pinned
    }

    pub fn unpin_local_variables(&self, names: Option<&[String]>) -> TypeEnv {
        let mut env = self.clone();
        for (name, (type_, _)) in &self.local_variable_types {
            ensure_local_variable_name(name);
            if names.is_none_or(|names| names.contains(name)) {
                env.local_variable_types
                    .insert(name.clone(), (type_.clone(), None));
            }
        }
        env
    }

    pub fn substitute(&self, substitution: &Substitution) -> TypeEnv {
        let mut env = self.clone();
        for (_, entry) in env.local_variable_types.iter_mut() {
            entry.0 = entry.0.substitute(substitution);
            entry.1 = entry.1.as_ref().map(|type_| type_.substitute(substitution));
        }
        env
    }

    pub fn join(&self, envs: &[TypeEnv]) -> TypeEnv {
        let mut all_names: Vec<String> = Vec::new();
        for env in envs {
            for name in env.local_variable_types.keys() {
                if !all_names.contains(name) {
                    all_names.push(name.clone());
                }
            }
        }
        let mut assignments = Vec::new();
        for name in all_names {
            let types: Vec<Type> = envs
                .iter()
                .map(|env| env.get(&name).unwrap_or(Type::Nil))
                .collect();
            let union = Type::union(types);
            if self.get(&name).as_ref() != Some(&union) {
                assignments.push((name, union));
            }
        }
        let mut common: Option<Vec<String>> = None;
        for env in envs {
            let keys: Vec<String> = env.pure_method_calls.keys().cloned().collect();
            common = Some(match common {
                None => keys,
                Some(previous) => previous
                    .into_iter()
                    .filter(|key| keys.contains(key))
                    .collect(),
            });
        }
        let mut updates = IndexMap::default();
        for key in common.unwrap_or_default() {
            let types: Vec<Type> = envs
                .iter()
                .map(|env| {
                    let pure = &env.pure_method_calls[&key];
                    pure.type_
                        .clone()
                        .unwrap_or_else(|| pure.call.return_type.clone())
                })
                .collect();
            let first = envs[0].pure_method_calls[&key].clone();
            updates.insert(
                key,
                PureCall {
                    type_: Some(Type::union(types)),
                    ..first
                },
            );
        }
        let mut env = self.assign_local_variables(&assignments);
        env.merge_pure_calls(updates);
        env
    }

    pub fn add_pure_call(
        &self,
        node: &Rc<Node>,
        call: Rc<MethodCall>,
        type_: Option<Type>,
    ) -> TypeEnv {
        let key = node_key(node);
        if let Some(existing) = self.pure_method_calls.get(&key)
            && existing.call.same_call(&call)
        {
            return self.clone();
        }
        let mut updates = self.pure_node_invalidation(&self.invalidated_pure_nodes(&key));
        updates.insert(
            key,
            PureCall {
                node: node.clone(),
                call,
                type_,
            },
        );
        let mut env = self.clone();
        env.merge_pure_calls(updates);
        env
    }

    pub fn replace_pure_call_type(&self, node: &Node, type_: Type) -> TypeEnv {
        let mut env = self.clone();
        if let Some(pure) = env.pure_method_calls.get_mut(&node_key(node)) {
            pure.type_ = Some(type_);
        }
        env
    }

    pub fn invalidate_pure_node(&self, node: &Node) -> TypeEnv {
        let mut env = self.clone();
        let invalidation =
            self.pure_node_invalidation(&self.invalidated_pure_nodes(&node_key(node)));
        env.merge_pure_calls(invalidation);
        env
    }

    fn pure_node_invalidation(&self, invalidated: &[String]) -> IndexMap<String, PureCall> {
        let mut invalidation = IndexMap::default();
        for key in invalidated {
            if let Some(pure) = self.pure_method_calls.get(key) {
                invalidation.insert(
                    key.clone(),
                    PureCall {
                        type_: None,
                        ..pure.clone()
                    },
                );
            }
        }
        invalidation
    }

    fn invalidated_pure_nodes(&self, invalidated_key: &str) -> Vec<String> {
        let mut invalidated = Vec::new();
        for (key, pure) in &self.pure_method_calls {
            let mut descendants = Vec::new();
            descendant_keys(&pure.node, &mut descendants);
            if descendants
                .iter()
                .any(|descendant| descendant == invalidated_key)
            {
                invalidated.push(key.clone());
            }
        }
        invalidated
    }
}
