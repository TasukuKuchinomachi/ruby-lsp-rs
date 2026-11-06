use std::{cell::RefCell, env, rc::Rc};

use rbs_definition::{
    ancestor_builder::{Ancestor, OneAncestors},
    ast::{Literal, TypeName, Variance},
};
use rbs_definition::{collections::HashMap, errors::RbsError};

use crate::{
    builder::{Builder, Config, Entry, Shape},
    method_type::{Block, Function, MethodType, Params, PositionalKind, TypeParam},
    substitution::Substitution,
    subtyping::{
        cache::{Cache, CacheKey},
        constraints::SharedConstraints,
        relation::{AnyRelation, MethodOrEntry, Relation},
        result::{All, Any, Checked, FailureError, SubtypingResult},
        variable_variance::VariableVariance,
    },
    types::{Type, fresh_variable_name, literal_back_type},
};

pub fn abort_limit() -> usize {
    env::var("STEEP_SUBTYPING_ABORT_LIMIT")
        .ok()
        .and_then(|limit| limit.parse().ok())
        .unwrap_or(50)
}

#[derive(Default)]
pub struct CheckState {
    cache: Cache,
    bounds: RefCell<Vec<VariableBounds>>,
    self_type: RefCell<Option<Type>>,
    instance_type: RefCell<Option<Type>>,
    class_type: RefCell<Option<Type>>,
    constraints: RefCell<Option<SharedConstraints>>,
    assumptions: RefCell<Option<Vec<Relation<Type>>>>,
}

impl CheckState {
    pub(crate) fn forget_fresh_variables(&self) {
        self.cache.forget_fresh_variables();
    }
}

pub struct Check<'builder> {
    pub builder: &'builder Builder,
    state: Rc<CheckState>,
}

enum ExpandedBlock {
    NoBlock,
    Relation(Box<Relation<Block>>),
    Failure(Rc<SubtypingResult>),
}

type VariableBounds = Vec<(String, Option<Type>)>;

struct Ensure<Cleanup: FnMut()>(Cleanup);

impl<Cleanup: FnMut()> Drop for Ensure<Cleanup> {
    fn drop(&mut self) {
        (self.0)()
    }
}

type ParamPairs = Result<Vec<(Type, Type)>, Rc<SubtypingResult>>;

fn instance_of<'type_>(type_: &'type_ Type, name: &str, arity: usize) -> Option<&'type_ Type> {
    match type_ {
        Type::NameInstance {
            name: type_name,
            args,
        } if *type_name == TypeName::parse(name) && args.len() == arity => Some(type_),
        _ => None,
    }
}

fn is_name(type_: &Type) -> bool {
    matches!(
        type_,
        Type::NameInstance { .. }
            | Type::Interface { .. }
            | Type::Alias { .. }
            | Type::Singleton(_)
    )
}

fn same_name_class(left: &Type, right: &Type) -> bool {
    match (left, right) {
        (Type::NameInstance { name, .. }, Type::NameInstance { name: other, .. })
        | (Type::Interface { name, .. }, Type::Interface { name: other, .. })
        | (Type::Alias { name, .. }, Type::Alias { name: other, .. })
        | (Type::Singleton(name), Type::Singleton(other)) => name == other,
        _ => false,
    }
}

fn arg_type(type_: &Type) -> bool {
    match type_ {
        Type::NameInstance { args, .. } | Type::Interface { args, .. } => !args.is_empty(),
        _ => false,
    }
}

fn args_of(type_: &Type) -> &[Type] {
    match type_ {
        Type::NameInstance { args, .. }
        | Type::Interface { args, .. }
        | Type::Alias { args, .. } => args,
        _ => &[],
    }
}

fn boolean_union() -> Type {
    Type::union(vec![
        Type::Literal(Literal::Bool(true)),
        Type::Literal(Literal::Bool(false)),
    ])
}

fn method_failure(error: FailureError, relation: impl Into<AnyRelation>) -> Rc<SubtypingResult> {
    SubtypingResult::failure(relation, error)
}

impl<'builder> Check<'builder> {
    pub fn new(builder: &'builder Builder) -> Check<'builder> {
        Check::with_state(builder, Rc::default())
    }

    pub fn with_state(builder: &'builder Builder, state: Rc<CheckState>) -> Check<'builder> {
        Check { builder, state }
    }

    pub fn with_context<Output>(
        &self,
        self_type: Option<Type>,
        instance_type: Option<Type>,
        class_type: Option<Type>,
        constraints: SharedConstraints,
        body: impl FnOnce() -> Output,
    ) -> Output {
        *self.state.self_type.borrow_mut() = self_type;
        *self.state.instance_type.borrow_mut() = instance_type;
        *self.state.class_type.borrow_mut() = class_type;
        *self.state.constraints.borrow_mut() = Some(constraints);
        *self.state.assumptions.borrow_mut() = Some(Vec::new());
        let _ensure = Ensure(|| {
            *self.state.self_type.borrow_mut() = None;
            *self.state.instance_type.borrow_mut() = None;
            *self.state.class_type.borrow_mut() = None;
            *self.state.constraints.borrow_mut() = None;
            *self.state.assumptions.borrow_mut() = None;
        });
        body()
    }

    pub fn push_variable_bounds<Output>(
        &self,
        bounds: VariableBounds,
        body: impl FnOnce() -> Output,
    ) -> Output {
        self.state.bounds.borrow_mut().push(bounds);
        let _ensure = Ensure(|| {
            self.state.bounds.borrow_mut().pop();
        });
        body()
    }

    pub fn push_type_params<Output>(
        &self,
        params: &[TypeParam],
        body: impl FnOnce() -> Output,
    ) -> Output {
        let mut bounds: VariableBounds = Vec::new();
        for param in params {
            match bounds.iter_mut().find(|(name, _)| *name == param.name) {
                Some(entry) => entry.1 = param.upper_bound.clone(),
                None => bounds.push((param.name.clone(), param.upper_bound.clone())),
            }
        }
        self.push_variable_bounds(bounds, body)
    }

    pub fn variable_upper_bound(&self, name: &str) -> Option<Type> {
        for bounds in self.state.bounds.borrow().iter().rev() {
            if let Some((_, bound)) = bounds.iter().find(|(variable, _)| variable == name) {
                return bound.clone();
            }
        }
        None
    }

    pub fn variable_upper_bounds(&self) -> HashMap<String, Type> {
        let mut merged: HashMap<String, Option<Type>> = HashMap::default();
        for bounds in self.state.bounds.borrow().iter() {
            for (name, bound) in bounds {
                merged.insert(name.clone(), bound.clone());
            }
        }
        merged
            .into_iter()
            .filter_map(|(name, bound)| bound.map(|bound| (name, bound)))
            .collect()
    }

    fn self_type(&self) -> Result<Type, RbsError> {
        self.state
            .self_type
            .borrow()
            .clone()
            .ok_or_else(|| "the self type of the subtyping check is not set".into())
    }

    fn instance_type(&self) -> Type {
        self.state
            .instance_type
            .borrow()
            .clone()
            .unwrap_or(Type::Instance)
    }

    fn class_type(&self) -> Type {
        self.state
            .class_type
            .borrow()
            .clone()
            .unwrap_or(Type::Class)
    }

    pub fn constraints(&self) -> Result<SharedConstraints, RbsError> {
        self.state
            .constraints
            .borrow()
            .clone()
            .ok_or_else(|| "the constraints of the subtyping check are not set".into())
    }

    fn assumptions_contain(&self, relation: &Relation<Type>) -> Result<bool, RbsError> {
        self.state
            .assumptions
            .borrow()
            .as_ref()
            .map(|assumptions| assumptions.contains(relation))
            .ok_or_else(|| "the assumptions of the subtyping check are not set".into())
    }

    fn assumptions_size(&self) -> Result<usize, RbsError> {
        self.state
            .assumptions
            .borrow()
            .as_ref()
            .map(Vec::len)
            .ok_or_else(|| "the assumptions of the subtyping check are not set".into())
    }

    fn push_assumption(
        &self,
        relation: &Relation<Type>,
        body: impl FnOnce() -> Checked,
    ) -> Checked {
        self.state
            .assumptions
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| "the assumptions of the subtyping check are not set".to_string())?
            .push(relation.clone());
        let result = body();
        if let Some(assumptions) = self.state.assumptions.borrow_mut().as_mut()
            && let Some(index) = assumptions
                .iter()
                .position(|assumption| assumption == relation)
        {
            assumptions.remove(index);
        }
        result
    }

    fn is_unknown(&self, name: &str) -> Result<bool, RbsError> {
        Ok(self.constraints()?.borrow().is_unknown(name))
    }

    fn add_constraint(
        &self,
        name: &str,
        sub_type: Option<&Type>,
        super_type: Option<&Type>,
    ) -> Result<(), RbsError> {
        self.constraints()?
            .borrow_mut()
            .add(name, sub_type, super_type, false)
    }

    fn each_ancestor(ancestors: &OneAncestors) -> Vec<&Ancestor> {
        ancestors
            .super_class
            .iter()
            .chain(ancestors.included_modules.iter().flatten())
            .chain(ancestors.included_interfaces.iter().flatten())
            .chain(ancestors.prepended_modules.iter().flatten())
            .chain(ancestors.extended_modules.iter().flatten())
            .chain(ancestors.extended_interfaces.iter().flatten())
            .collect()
    }

    fn ancestor_type(&self, ancestor: &Ancestor, substitution: Option<&Substitution>) -> Type {
        match ancestor {
            Ancestor::Instance { name, args, .. } => {
                let args: Vec<Type> = args
                    .iter()
                    .map(|arg| {
                        let type_ = self.builder.factory.type_(arg);
                        match substitution {
                            Some(substitution) => type_.substitute(substitution),
                            None => type_,
                        }
                    })
                    .collect();
                if name.is_class() {
                    Type::NameInstance {
                        name: name.clone(),
                        args,
                    }
                } else {
                    Type::Interface {
                        name: name.clone(),
                        args,
                    }
                }
            }
            Ancestor::Singleton { name } => Type::Singleton(name.clone()),
        }
    }

    pub fn instance_super_types(
        &self,
        type_name: &TypeName,
        args: &[Type],
    ) -> Result<Vec<Type>, RbsError> {
        let ancestors = self
            .builder
            .definition_builder
            .ancestor_builder
            .one_instance_ancestors(type_name)?;
        let substitution = (!args.is_empty())
            .then(|| Substitution::build(&ancestors.params, args, None, None, None));
        Ok(Self::each_ancestor(&ancestors)
            .into_iter()
            .map(|ancestor| self.ancestor_type(ancestor, substitution.as_ref()))
            .collect())
    }

    pub fn singleton_super_types(&self, type_name: &TypeName) -> Result<Vec<Type>, RbsError> {
        let ancestors = self
            .builder
            .definition_builder
            .ancestor_builder
            .one_singleton_ancestors(type_name)?;
        Ok(ancestors
            .each_ancestor()
            .map(|ancestor| self.ancestor_type(ancestor, None))
            .collect())
    }

    pub fn check(
        &self,
        relation: Relation<Type>,
        constraints: SharedConstraints,
        self_type: Option<Type>,
        instance_type: Option<Type>,
        class_type: Option<Type>,
    ) -> Checked {
        self.with_context(self_type, instance_type, class_type, constraints, || {
            self.check_type(relation)
        })
    }

    fn cache_bounds(&self, relation: &Relation<Type>) -> Vec<(String, Type)> {
        let mut variables = relation.sub_type.free_variables();
        relation.super_type.collect_free_variables(&mut variables);
        let mut bounds: Vec<(String, Type)> = variables
            .into_iter()
            .filter_map(|variable| {
                self.variable_upper_bound(&variable)
                    .map(|bound| (variable, bound))
            })
            .collect();
        bounds.sort_by(|left, right| left.0.cmp(&right.0));
        bounds
    }

    pub fn check_type(&self, relation: Relation<Type>) -> Checked {
        if self.assumptions_size()? > abort_limit() {
            return Ok(SubtypingResult::failure(relation, FailureError::LoopAbort));
        }
        let key = CacheKey {
            relation: relation.clone(),
            self_type: self.state.self_type.borrow().clone(),
            instance_type: self.state.instance_type.borrow().clone(),
            class_type: self.state.class_type.borrow().clone(),
            bounds: self.cache_bounds(&relation),
        };
        if let Some(cached) = self.state.cache.get(&key) {
            let mut variables = relation.sub_type.free_variables();
            relation.super_type.collect_free_variables(&mut variables);
            let mut has_unknown = false;
            for variable in &variables {
                if self.is_unknown(variable)? {
                    has_unknown = true;
                    break;
                }
            }
            if !has_unknown {
                return Ok(cached);
            }
        }
        if self.assumptions_contain(&relation)? {
            return Ok(SubtypingResult::success(relation));
        }
        let result = self.push_assumption(&relation, || self.check_type0(relation.clone()))?;
        self.state.cache.insert(key, result.clone());
        Ok(result)
    }

    fn sorted_by_hole_path<'a>(&self, types: &'a [Type]) -> Result<Vec<&'a Type>, RbsError> {
        let mut keyed: Vec<(Option<usize>, &Type)> = types
            .iter()
            .map(|type_| Ok((self.hole_path(type_)?.map(|path| path.len()), type_)))
            .collect::<Result<_, RbsError>>()?;
        keyed.sort_by(|(left, _), (right, _)| match (left, right) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(left), Some(right)) => right.cmp(left),
        });
        Ok(keyed.into_iter().map(|(_, type_)| type_).collect())
    }

    fn config(&self, type_: &Type) -> Config {
        Config {
            self_type: Some(type_.clone()),
            class_type: None,
            instance_type: None,
            variable_bounds: self.variable_upper_bounds(),
        }
    }

    fn public_shape(&self, type_: &Type) -> Result<Option<Shape>, RbsError> {
        Ok(self
            .builder
            .shape(type_, &self.config(type_))?
            .map(|shape| shape.public_shape()))
    }

    fn shape_relation(
        &self,
        relation: &Relation<Type>,
    ) -> Result<Option<Relation<Shape>>, RbsError> {
        let Some(sub_shape) = self.public_shape(&relation.sub_type)? else {
            return Ok(None);
        };
        let Some(super_shape) = self.public_shape(&relation.super_type)? else {
            return Ok(None);
        };
        Ok(Some(Relation::new(sub_shape, super_shape)))
    }

    fn unknown_pair(relation: Relation<Type>) -> Rc<SubtypingResult> {
        SubtypingResult::failure(relation.clone(), FailureError::UnknownPair { relation })
    }

    fn check_type0(&self, relation: Relation<Type>) -> Checked {
        let Relation {
            sub_type,
            super_type,
        } = &relation;

        if self.same_type(&relation)? {
            return Ok(SubtypingResult::success(relation));
        }
        if matches!(sub_type, Type::Any) || matches!(super_type, Type::Any) {
            return Ok(SubtypingResult::success(relation));
        }
        if matches!(super_type, Type::Void | Type::Top) || matches!(sub_type, Type::Bot) {
            return Ok(SubtypingResult::success(relation));
        }
        if matches!(super_type, Type::Boolean) {
            let child = self.check_type(Relation::new(sub_type.clone(), boolean_union()))?;
            return Ok(SubtypingResult::expand(relation, child));
        }
        if matches!(sub_type, Type::Boolean | Type::Logic(_)) {
            let child = self.check_type(Relation::new(boolean_union(), super_type.clone()))?;
            return Ok(SubtypingResult::expand(relation, child));
        }
        let instance_type = self.instance_type();
        if matches!(sub_type, Type::Instance) && !matches!(instance_type, Type::Instance) {
            let child = self.check_type(Relation::new(instance_type, super_type.clone()))?;
            return Ok(SubtypingResult::expand(relation, child));
        }
        if matches!(super_type, Type::Instance) && !matches!(instance_type, Type::Instance) {
            let mut all = All::new(relation.clone());
            let instance_relation = Relation::new(sub_type.clone(), instance_type);
            all.add(instance_relation.clone(), || {
                self.check_type(instance_relation.clone())
            })?;
            let flipped = instance_relation.flip();
            all.add(flipped.clone(), || self.check_type(flipped.clone()))?;
            return Ok(all.finish());
        }
        if matches!(sub_type, Type::Class) && !matches!(instance_type, Type::Class) {
            let child = self.check_type(Relation::new(self.class_type(), super_type.clone()))?;
            return Ok(SubtypingResult::expand(relation, child));
        }
        if matches!(super_type, Type::Class) && !matches!(instance_type, Type::Class) {
            let mut all = All::new(relation.clone());
            let class_relation = Relation::new(sub_type.clone(), self.class_type());
            all.add(class_relation.clone(), || {
                self.check_type(class_relation.clone())
            })?;
            let flipped = class_relation.flip();
            all.add(flipped.clone(), || self.check_type(flipped.clone()))?;
            return Ok(all.finish());
        }
        if let Type::Alias { name, args } = sub_type {
            let expanded = self.builder.factory.expand_alias(name, args)?;
            let child = self.check_type(Relation::new(expanded, super_type.clone()))?;
            return Ok(SubtypingResult::expand(relation, child));
        }
        if let Type::Alias { name, args } = super_type {
            let expanded = self.builder.factory.expand_alias(name, args)?;
            let child = self.check_type(Relation::new(sub_type.clone(), expanded))?;
            return Ok(SubtypingResult::expand(relation, child));
        }
        if let Type::Var(name) = super_type
            && self.is_unknown(name)?
        {
            return match self.variable_upper_bound(name) {
                Some(upper_bound) => {
                    let child = self.check_type(Relation::new(sub_type.clone(), upper_bound))?;
                    let result = SubtypingResult::expand(relation.clone(), child);
                    if result.is_success() {
                        self.add_constraint(name, Some(sub_type), None)?;
                    }
                    Ok(result)
                }
                None => {
                    self.add_constraint(name, Some(sub_type), None)?;
                    Ok(SubtypingResult::success(relation))
                }
            };
        }
        if let Type::Var(name) = sub_type
            && self.is_unknown(name)?
        {
            self.add_constraint(name, None, Some(super_type))?;
            return Ok(SubtypingResult::success(relation));
        }
        if let Type::Var(name) = sub_type
            && let Some(upper_bound) = self.variable_upper_bound(name)
        {
            let child = self.check_type(Relation::new(upper_bound, super_type.clone()))?;
            return Ok(SubtypingResult::expand(relation, child));
        }
        if let (Type::Intersection(sub_types), Type::Union(super_types)) = (sub_type, super_type) {
            let mut base_result = Any::new(relation.clone());
            base_result.add(relation.clone(), || {
                let mut result = Any::new(relation.clone());
                for super_type in self.sorted_by_hole_path(super_types)? {
                    let member = Relation::new(sub_type.clone(), super_type.clone());
                    result.add(member.clone(), || self.check_type(member.clone()))?;
                }
                Ok(result.finish())
            })?;
            base_result.add(relation.clone(), || {
                let mut result = Any::new(relation.clone());
                for sub_type in self.sorted_by_hole_path(sub_types)? {
                    let member = Relation::new(sub_type.clone(), super_type.clone());
                    result.add(member.clone(), || self.check_type(member.clone()))?;
                }
                Ok(result.finish())
            })?;
            return Ok(base_result.finish());
        }
        if let Type::Intersection(super_types) = super_type {
            let mut all = All::new(relation.clone());
            for super_type in super_types {
                let member = Relation::new(sub_type.clone(), super_type.clone());
                all.add(member.clone(), || self.check_type(member.clone()))?;
            }
            return Ok(all.finish());
        }
        if let Type::Union(sub_types) = sub_type {
            let mut all = All::new(relation.clone());
            for sub_type in sub_types {
                let member = Relation::new(sub_type.clone(), super_type.clone());
                all.add(member.clone(), || self.check_type(member.clone()))?;
            }
            return Ok(all.finish());
        }
        if let Type::Intersection(sub_types) = sub_type {
            let mut any = Any::new(relation.clone());
            for sub_type in self.sorted_by_hole_path(sub_types)? {
                let member = Relation::new(sub_type.clone(), super_type.clone());
                any.add(member.clone(), || self.check_type(member.clone()))?;
            }
            return Ok(any.finish());
        }
        if let Type::Union(super_types) = super_type {
            let mut any = Any::new(relation.clone());
            for super_type in self.sorted_by_hole_path(super_types)? {
                let member = Relation::new(sub_type.clone(), super_type.clone());
                any.add(member.clone(), || self.check_type(member.clone()))?;
            }
            return Ok(any.finish());
        }
        if matches!(sub_type, Type::SelfType) {
            let self_type = self.self_type()?;
            if !matches!(self_type, Type::SelfType) {
                let child = self.check_type(Relation::new(self_type, super_type.clone()))?;
                return Ok(SubtypingResult::expand(relation, child));
            }
        }
        if matches!(super_type, Type::Interface { .. }) {
            let Some(shape_relation) = self.shape_relation(&relation)? else {
                return Ok(Self::unknown_pair(relation));
            };
            let child = self.check_interface(shape_relation)?;
            return Ok(SubtypingResult::expand(relation, child));
        }
        if is_name(sub_type) && is_name(super_type) {
            if same_name_class(sub_type, super_type) {
                if arg_type(sub_type) && arg_type(super_type) {
                    return self.check_type_arg(relation);
                }
                return Ok(SubtypingResult::success(relation));
            }
            let possible_sub_types = match sub_type {
                Type::NameInstance { name, args } => self.instance_super_types(name, args)?,
                Type::Singleton(name) => self.singleton_super_types(name)?,
                _ => Vec::new(),
            };
            if possible_sub_types.is_empty() {
                return Ok(Self::unknown_pair(relation));
            }
            let mut any = Any::new(relation.clone());
            for possible_sub_type in possible_sub_types {
                let member = Relation::new(possible_sub_type, super_type.clone());
                any.add(member.clone(), || self.check_type(member.clone()))?;
            }
            return Ok(any.finish());
        }
        if let (
            Type::Proc {
                function: sub_function,
                block: sub_block,
                self_type: sub_self_type,
            },
            Type::Proc {
                function: super_function,
                block: super_block,
                self_type: super_self_type,
            },
        ) = (sub_type, super_type)
        {
            let name = "__proc__";
            let mut all = All::new(relation.clone());
            let function_relation =
                Relation::new((**sub_function).clone(), (**super_function).clone());
            all.add(function_relation.clone(), || {
                self.check_function(name, function_relation.clone())
            })?;
            all.add_result(self.check_self_type_binding(
                relation.clone().into(),
                sub_self_type.as_deref(),
                super_self_type.as_deref(),
            )?);
            let block_relation = Relation::new(
                sub_block.as_deref().cloned(),
                super_block.as_deref().cloned(),
            );
            all.add_optional(block_relation.clone(), || {
                match self.expand_block_given(name, block_relation.clone()) {
                    ExpandedBlock::Relation(block_relation) => {
                        let block_relation = *block_relation;
                        let mut all = All::new(block_relation.clone());
                        all.add_result(self.check_self_type_binding(
                            block_relation.clone().into(),
                            block_relation.super_type.self_type.as_ref(),
                            block_relation.sub_type.self_type.as_ref(),
                        )?);
                        let function_relation = block_relation.map(|block| block.type_.clone());
                        all.add(function_relation.clone(), || {
                            self.check_function(name, function_relation.clone().flip())
                        })?;
                        Ok(Some(all.finish()))
                    }
                    ExpandedBlock::Failure(failure) => Ok(Some(failure)),
                    ExpandedBlock::NoBlock => Ok(None),
                }
            })?;
            return Ok(all.finish());
        }
        if let (Type::Tuple(sub_types), Type::Tuple(super_types)) = (sub_type, super_type) {
            if sub_types.len() != super_types.len() {
                return Ok(Self::unknown_pair(relation));
            }
            let mut all = All::new(relation.clone());
            for (sub_type, super_type) in sub_types.iter().zip(super_types) {
                let member = Relation::new(sub_type.clone(), super_type.clone());
                all.add(member.clone(), || self.check_type(member.clone()))?;
            }
            return Ok(all.finish());
        }
        if let Type::Tuple(sub_types) = sub_type
            && let Some(Type::NameInstance { args, .. }) = instance_of(super_type, "::Array", 1)
        {
            let element_type = Type::union(sub_types.clone());
            let child = self.check_type(Relation::new(element_type, args[0].clone()))?;
            return Ok(SubtypingResult::expand(relation, child));
        }
        if let Type::Tuple(sub_types) = sub_type {
            let mut any = Any::new(relation.clone());
            let array_type = Type::NameInstance {
                name: TypeName::parse("::Array"),
                args: vec![Type::union(sub_types.clone())],
            };
            let array_relation = Relation::new(array_type, super_type.clone());
            any.add(array_relation.clone(), || {
                self.check_type(array_relation.clone())
            })?;
            if let Some(shape_relation) = self.shape_relation(&relation)? {
                any.add(shape_relation.clone(), || {
                    self.check_interface(shape_relation.clone())
                })?;
            }
            return Ok(any.finish());
        }
        if let (Type::Record(sub_elements), Type::Record(super_elements)) = (sub_type, super_type) {
            let mut all = All::new(relation.clone());
            let mut unchecked_keys: Vec<String> = sub_elements
                .iter()
                .map(|element| element.key.inspect())
                .collect();
            for super_element in super_elements {
                let key = super_element.key.inspect();
                let sub_element = sub_elements
                    .iter()
                    .find(|element| element.key.inspect() == key);
                if super_element.required {
                    let member = Relation::new(
                        sub_element.map_or(Type::Nil, |element| element.type_.clone()),
                        super_element.type_.clone(),
                    );
                    all.add(member.clone(), || self.check_type(member.clone()))?;
                } else if let Some(sub_element) = sub_element {
                    let member =
                        Relation::new(sub_element.type_.clone(), super_element.type_.clone());
                    all.add(member.clone(), || self.check_type(member.clone()))?;
                }
                unchecked_keys.retain(|unchecked| *unchecked != key);
            }
            if !unchecked_keys.is_empty() {
                return Ok(Self::unknown_pair(relation));
            }
            return Ok(all.finish());
        }
        if let Type::Record(sub_elements) = sub_type {
            let mut any = Any::new(relation.clone());
            let key_type = Type::union(
                sub_elements
                    .iter()
                    .map(|element| Type::Literal(element.key.clone()))
                    .collect(),
            );
            let value_type = Type::union(
                sub_elements
                    .iter()
                    .map(|element| element.type_.clone())
                    .collect(),
            );
            let hash_type = Type::NameInstance {
                name: TypeName::parse("::Hash"),
                args: vec![key_type, value_type],
            };
            let hash_relation = Relation::new(hash_type, super_type.clone());
            any.add(hash_relation.clone(), || {
                self.check_type(hash_relation.clone())
            })?;
            if let Some(shape_relation) = self.shape_relation(&relation)? {
                any.add(shape_relation.clone(), || {
                    self.check_interface(shape_relation.clone())
                })?;
            }
            return Ok(any.finish());
        }
        if matches!(sub_type, Type::Proc { .. }) && instance_of(super_type, "::Proc", 0).is_some() {
            return Ok(SubtypingResult::success(relation));
        }
        if let Type::Literal(literal) = super_type {
            return Ok(match literal {
                Literal::Bool(true) if instance_of(sub_type, "::TrueClass", 0).is_some() => {
                    SubtypingResult::success(relation)
                }
                Literal::Bool(false) if instance_of(sub_type, "::FalseClass", 0).is_some() => {
                    SubtypingResult::success(relation)
                }
                _ => Self::unknown_pair(relation),
            });
        }
        if matches!(super_type, Type::Nil) && instance_of(sub_type, "::NilClass", 0).is_some() {
            return Ok(SubtypingResult::success(relation));
        }
        if let Type::Literal(literal) = sub_type {
            let child = self.check_type(Relation::new(
                literal_back_type(literal),
                super_type.clone(),
            ))?;
            return Ok(SubtypingResult::expand(relation, child));
        }
        Ok(Self::unknown_pair(relation))
    }

    fn type_params_of(&self, type_: &Type) -> Result<Vec<Variance>, RbsError> {
        let definition = match type_ {
            Type::NameInstance { name, .. } => {
                self.builder.definition_builder.build_instance(name)?
            }
            Type::Interface { name, .. } => {
                self.builder.definition_builder.build_interface(name)?
            }
            _ => return Err(format!("no definition for `{type_}`").into()),
        };
        Ok(definition
            .type_parameters
            .iter()
            .map(|parameter| parameter.variance)
            .collect())
    }

    fn check_type_arg(&self, relation: Relation<Type>) -> Checked {
        let sub_args = args_of(&relation.sub_type);
        let super_args = args_of(&relation.super_type);
        let variances = self.type_params_of(&relation.super_type)?;
        let mut all = All::new(relation.clone());
        for (index, sub_arg) in sub_args.iter().enumerate() {
            let super_arg = super_args
                .get(index)
                .ok_or_else(|| format!("too few type arguments: {relation}"))?;
            let variance = variances
                .get(index)
                .ok_or_else(|| format!("too few type parameters: {relation}"))?;
            match variance {
                Variance::Covariant => {
                    let member = Relation::new(sub_arg.clone(), super_arg.clone());
                    all.add(member.clone(), || self.check_type(member.clone()))?;
                }
                Variance::Contravariant => {
                    let member = Relation::new(super_arg.clone(), sub_arg.clone());
                    all.add(member.clone(), || self.check_type(member.clone()))?;
                }
                Variance::Invariant => {
                    let member = Relation::new(sub_arg.clone(), super_arg.clone());
                    all.add(member.clone(), || self.check_type(member.clone()))?;
                    let flipped = member.flip();
                    all.add(flipped.clone(), || self.check_type(flipped.clone()))?;
                }
            }
        }
        Ok(all.finish())
    }

    fn same_type(&self, relation: &Relation<Type>) -> Result<bool, RbsError> {
        if self.assumptions_contain(relation)?
            && self.assumptions_contain(&relation.clone().flip())?
        {
            return Ok(true);
        }
        let factory = &self.builder.factory;
        Ok(factory.normalize_type(&relation.sub_type)
            == factory.normalize_type(&relation.super_type))
    }

    pub fn check_interface(&self, relation: Relation<Shape>) -> Checked {
        let Relation {
            sub_type: sub_interface,
            super_type: super_interface,
        } = &relation;
        let mut method_pairs: Vec<(String, Relation<Entry>)> = Vec::new();
        for method_name in super_interface.method_names() {
            let Some(super_method) = super_interface.method(self.builder, &method_name)? else {
                continue;
            };
            match sub_interface.method(self.builder, &method_name)? {
                Some(sub_method) => {
                    method_pairs.push((method_name, Relation::new(sub_method, super_method)))
                }
                None => {
                    return Ok(SubtypingResult::failure(
                        relation.clone(),
                        FailureError::MethodMissing { name: method_name },
                    ));
                }
            }
        }
        let mut all = All::new(relation.clone());
        for (method_name, method_relation) in method_pairs {
            all.add(method_relation.clone(), || {
                self.check_method(&method_name, method_relation.clone())
            })?;
        }
        Ok(all.finish())
    }

    fn check_method(&self, name: &str, relation: Relation<Entry>) -> Checked {
        let Relation {
            sub_type: sub_method,
            super_type: super_method,
        } = &relation;
        let mut all = All::new(relation.clone());
        for super_type in super_method.method_types() {
            let method_relation = Relation::new(
                MethodOrEntry::Entry(sub_method.clone()),
                MethodOrEntry::MethodType(Box::new(super_type.clone())),
            );
            all.add(method_relation.clone(), || {
                let mut any = Any::new(method_relation.clone());
                for sub_type in sub_method.method_types() {
                    let pair = Relation::new(sub_type.clone(), super_type.clone());
                    any.add(pair.clone(), || {
                        self.check_generic_method_type(name, pair.clone())
                    })?;
                }
                Ok(any.finish())
            })?;
        }
        Ok(all.finish())
    }

    fn fresh_variables(type_params: &[TypeParam]) -> Vec<Type> {
        type_params
            .iter()
            .map(|parameter| Type::Var(fresh_variable_name(&parameter.name)))
            .collect()
    }

    fn variable_names(types: &[Type]) -> Vec<String> {
        types
            .iter()
            .map(|type_| match type_ {
                Type::Var(name) => name.clone(),
                _ => unreachable!("fresh variables are type variables"),
            })
            .collect()
    }

    fn instantiate_with(method_type: &MethodType, args: &[Type]) -> MethodType {
        let names: Vec<String> = method_type
            .type_params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        method_type.instantiate(&Substitution::build(&names, args, None, None, None))
    }

    fn check_generic_method_type(&self, name: &str, relation: Relation<MethodType>) -> Checked {
        let Relation {
            sub_type,
            super_type,
        } = &relation;
        match (
            sub_type.type_params.is_empty(),
            super_type.type_params.is_empty(),
        ) {
            (true, true) => self.check_method_type(name, relation),
            (false, true) => {
                let sub_args = Self::fresh_variables(&sub_type.type_params);
                let instantiated_sub_type = Self::instantiate_with(sub_type, &sub_args);
                let sub_variables = Self::variable_names(&sub_args);
                for variable in &sub_variables {
                    self.constraints()?.borrow_mut().add_unknown(variable);
                }
                let instantiated_relation =
                    Relation::new(instantiated_sub_type.clone(), super_type.clone());
                let mut all = All::new(instantiated_relation.clone());
                for (arg, parameter) in sub_args.iter().zip(&sub_type.type_params) {
                    if let Some(upper_bound) = &parameter.upper_bound {
                        let member = Relation::new(arg.clone(), upper_bound.clone());
                        all.add(member.clone(), || self.check_type(member.clone()))?;
                    }
                }
                match self.match_method_type_fails(name, &instantiated_sub_type, super_type) {
                    Some(failure) => {
                        all.add_result(Some(failure));
                    }
                    None => {
                        all.add_result(Some(
                            self.check_method_type(name, instantiated_relation.clone())?,
                        ));
                    }
                }
                all.add(instantiated_relation.clone(), || {
                    self.check_constraints(
                        instantiated_relation.clone().into(),
                        &sub_variables,
                        &VariableVariance::from_method_type(&instantiated_sub_type),
                    )
                })?;
                Ok(SubtypingResult::expand(relation, all.finish()))
            }
            (true, false) => {
                let mut all = All::new(relation.clone());
                let super_args = Self::fresh_variables(&super_type.type_params);
                let instantiated_super_type = Self::instantiate_with(super_type, &super_args);
                match self.match_method_type_fails(name, sub_type, &instantiated_super_type) {
                    Some(failure) => {
                        all.add_result(Some(failure));
                    }
                    None => {
                        let super_variables = Self::variable_names(&super_args);
                        for variable in &super_variables {
                            self.constraints()?.borrow_mut().add_unknown(variable);
                        }
                        let instantiated_relation =
                            Relation::new(sub_type.clone(), instantiated_super_type);
                        all.add(instantiated_relation.clone(), || {
                            let result =
                                self.check_method_type(name, instantiated_relation.clone())?;
                            let mut constrained = false;
                            for variable in &super_variables {
                                if self.constraints()?.borrow().has_constraint(variable)? {
                                    constrained = true;
                                    break;
                                }
                            }
                            if result.is_success() && !constrained {
                                Ok(result)
                            } else {
                                Ok(method_failure(
                                    FailureError::PolyMethodSubtyping {
                                        name: name.to_string(),
                                    },
                                    instantiated_relation.clone(),
                                ))
                            }
                        })?;
                    }
                }
                Ok(all.finish())
            }
            (false, false) if sub_type.type_params.len() == super_type.type_params.len() => {
                let mut all = All::new(relation.clone());
                let args = Self::fresh_variables(&sub_type.type_params);
                let variables = Self::variable_names(&args);
                for variable in &variables {
                    self.constraints()?.borrow_mut().add_unknown(variable);
                }
                let mut upper_bounds: VariableBounds = Vec::new();
                let mut relations: Vec<Relation<Type>> = Vec::new();
                for ((variable, sub_parameter), super_parameter) in variables
                    .iter()
                    .zip(&sub_type.type_params)
                    .zip(&super_type.type_params)
                {
                    match (&sub_parameter.upper_bound, &super_parameter.upper_bound) {
                        (Some(sub_upper_bound), Some(super_upper_bound)) => {
                            upper_bounds.push((variable.clone(), Some(sub_upper_bound.clone())));
                            relations.push(Relation::new(
                                sub_upper_bound.clone(),
                                super_upper_bound.clone(),
                            ));
                        }
                        (Some(sub_upper_bound), None) => {
                            upper_bounds.push((variable.clone(), Some(sub_upper_bound.clone())));
                        }
                        (None, Some(_)) => {
                            let missing = AnyRelation::MissingUpperBound(Type::Var(
                                sub_parameter.name.clone(),
                            ));
                            all.add(missing.clone(), || {
                                Ok(method_failure(
                                    FailureError::PolyMethodSubtyping {
                                        name: name.to_string(),
                                    },
                                    missing.clone(),
                                ))
                            })?;
                        }
                        (None, None) => {}
                    }
                }
                self.push_variable_bounds(upper_bounds, || -> Result<(), RbsError> {
                    let instantiated_sub_type = Self::instantiate_with(sub_type, &args);
                    let instantiated_super_type = Self::instantiate_with(super_type, &args);
                    for member in relations {
                        all.add(member.clone(), || self.check_type(member.clone()))?;
                    }
                    let instantiated_relation =
                        Relation::new(instantiated_sub_type, instantiated_super_type);
                    all.add(instantiated_relation.clone(), || {
                        self.check_method_type(name, instantiated_relation.clone())
                    })?;
                    Ok(())
                })?;
                Ok(all.finish())
            }
            (false, false) => Ok(method_failure(
                FailureError::PolyMethodSubtyping {
                    name: name.to_string(),
                },
                relation.clone(),
            )),
        }
    }

    fn check_constraints(
        &self,
        relation: AnyRelation,
        variables: &[String],
        variance: &VariableVariance,
    ) -> Checked {
        let checker = Check::new(self.builder);
        let solution = self.constraints()?.borrow().solution(
            &checker,
            variance,
            variables,
            Some(self.self_type()?),
            Some(self.instance_type()),
            Some(self.class_type()),
        )?;
        Ok(match solution {
            Ok(_) => SubtypingResult::success(relation),
            Err(error) => SubtypingResult::failure(
                relation,
                FailureError::UnsatisfiedConstraints(Box::new(error)),
            ),
        })
    }

    pub fn check_method_type(&self, name: &str, relation: Relation<MethodType>) -> Checked {
        let Relation {
            sub_type,
            super_type,
        } = &relation;
        if !sub_type.type_params.is_empty() {
            return Err(format!("Expected monomorphic method type: {sub_type}").into());
        }
        if !super_type.type_params.is_empty() {
            return Err(format!("Expected monomorphic method type: {super_type}").into());
        }
        let mut all = All::new(relation.clone());
        let type_relation = Relation::new(sub_type.type_.clone(), super_type.type_.clone());
        match self.expand_block_given(
            name,
            Relation::new(sub_type.block.clone(), super_type.block.clone()),
        ) {
            ExpandedBlock::NoBlock => {
                all.add(type_relation.clone(), || {
                    self.check_function(name, type_relation.clone())
                })?;
            }
            ExpandedBlock::Relation(block_relation) => {
                let block_relation = *block_relation;
                all.add(type_relation.clone(), || {
                    self.check_function(name, type_relation.clone())
                })?;
                all.add(block_relation.clone(), || {
                    let mut all = All::new(block_relation.clone());
                    all.add_result(self.check_self_type_binding(
                        block_relation.clone().into(),
                        block_relation.super_type.self_type.as_ref(),
                        block_relation.sub_type.self_type.as_ref(),
                    )?);
                    let function_relation = Relation::new(
                        block_relation.super_type.type_.clone(),
                        block_relation.sub_type.type_.clone(),
                    );
                    all.add(function_relation.clone(), || {
                        self.check_function(name, function_relation.clone())
                    })?;
                    Ok(all.finish())
                })?;
            }
            ExpandedBlock::Failure(failure) => {
                let failure_relation = failure.relation().clone();
                all.add(failure_relation, || Ok(failure))?;
            }
        }
        Ok(all.finish())
    }

    fn expand_block_given(&self, name: &str, relation: Relation<Option<Block>>) -> ExpandedBlock {
        match (&relation.sub_type, &relation.super_type) {
            (None, None) => ExpandedBlock::NoBlock,
            (Some(sub_block), Some(super_block)) if super_block.optional == sub_block.optional => {
                ExpandedBlock::Relation(Box::new(Relation::new(
                    sub_block.clone(),
                    super_block.clone(),
                )))
            }
            (Some(sub_block), _) if sub_block.optional => ExpandedBlock::NoBlock,
            _ => ExpandedBlock::Failure(method_failure(
                FailureError::BlockMismatch {
                    name: name.to_string(),
                },
                relation,
            )),
        }
    }

    fn check_function(&self, name: &str, relation: Relation<Function>) -> Checked {
        let mut all = All::new(relation.clone());
        if let (Some(sub_params), Some(super_params)) =
            (&relation.sub_type.params, &relation.super_type.params)
        {
            let params_relation = Relation::new(sub_params.clone(), super_params.clone());
            all.add(params_relation.clone(), || {
                self.check_method_params(name, params_relation.clone())
            })?;
        }
        let return_relation = relation.map(|function| function.return_type.clone());
        all.add(return_relation.clone(), || {
            self.check_type(return_relation.clone())
        })?;
        Ok(all.finish())
    }

    fn check_self_type_binding(
        &self,
        relation: AnyRelation,
        sub_self: Option<&Type>,
        super_self: Option<&Type>,
    ) -> Result<Option<Rc<SubtypingResult>>, RbsError> {
        match (sub_self, super_self) {
            (None, None) => Ok(None),
            (Some(sub_self), Some(super_self)) => Ok(Some(
                self.check_type(Relation::new(super_self.clone(), sub_self.clone()))?,
            )),
            (Some(Type::Top), None) => Ok(None),
            _ => Ok(Some(SubtypingResult::failure(
                relation,
                FailureError::SelfBindingMismatch,
            ))),
        }
    }

    pub fn check_method_params(&self, name: &str, relation: Relation<Params>) -> Checked {
        match self.match_params(name, &relation) {
            Ok(pairs) => {
                if pairs.is_empty() {
                    return Ok(SubtypingResult::success(relation));
                }
                let mut all = All::new(relation.clone());
                for (sub_type, super_type) in pairs {
                    let member = Relation::new(super_type, sub_type);
                    all.add(member.clone(), || self.check_type(member.clone()))?;
                }
                Ok(all.finish())
            }
            Err(failure) => Ok(failure),
        }
    }

    fn match_method_type_fails(
        &self,
        name: &str,
        type1: &MethodType,
        type2: &MethodType,
    ) -> Option<Rc<SubtypingResult>> {
        if let (Some(params1), Some(params2)) = (&type1.type_.params, &type2.type_.params)
            && let Err(failure) =
                self.match_params(name, &Relation::new(params1.clone(), params2.clone()))
        {
            return Some(failure);
        }
        match self.expand_block_given(
            name,
            Relation::new(type1.block.clone(), type2.block.clone()),
        ) {
            ExpandedBlock::Failure(failure) => Some(failure),
            ExpandedBlock::Relation(relation) => {
                let relation = *relation;
                if let (Some(params1), Some(params2)) = (
                    &relation.sub_type.type_.params,
                    &relation.super_type.type_.params,
                ) && let Err(failure) =
                    self.match_params(name, &Relation::new(params1.clone(), params2.clone()))
                {
                    return Some(failure);
                }
                None
            }
            ExpandedBlock::NoBlock => None,
        }
    }

    fn match_params(&self, name: &str, relation: &Relation<Params>) -> ParamPairs {
        let Relation {
            sub_type: sub_params,
            super_type: super_params,
        } = relation;
        let failure = || {
            method_failure(
                FailureError::ParameterMismatch {
                    name: name.to_string(),
                },
                relation.clone(),
            )
        };
        let mut pairs: Vec<(Type, Type)> = Vec::new();
        let mut sub_flat = sub_params.flat_unnamed_params().into_iter();
        let mut super_flat = super_params.flat_unnamed_params().into_iter();
        let sub_rest = sub_params.rest_type();
        let super_rest = super_params.rest_type();

        if let Some(super_rest) = super_rest {
            let Some(sub_rest) = sub_rest else {
                return Err(failure());
            };
            for (_, sub_type) in sub_flat.by_ref() {
                match super_flat.next() {
                    Some((_, super_type)) => pairs.push((sub_type.clone(), super_type.clone())),
                    None => pairs.push((sub_type.clone(), super_rest.clone())),
                }
            }
            pairs.push((sub_rest.clone(), super_rest.clone()));
        } else if let Some(sub_rest) = sub_rest {
            for (_, sub_type) in sub_flat.by_ref() {
                match super_flat.next() {
                    Some((_, super_type)) => pairs.push((sub_type.clone(), super_type.clone())),
                    None => break,
                }
            }
            for (_, super_type) in super_flat.by_ref() {
                pairs.push((sub_rest.clone(), super_type.clone()));
            }
        } else if sub_params.required_types().len() + sub_params.optional_types().len()
            >= super_params.required_types().len() + super_params.optional_types().len()
        {
            for (kind, sub_type) in sub_flat.by_ref() {
                match super_flat.next() {
                    Some((_, super_type)) => pairs.push((sub_type.clone(), super_type.clone())),
                    None => {
                        if kind == PositionalKind::Required {
                            return Err(failure());
                        }
                        break;
                    }
                }
            }
        } else {
            return Err(failure());
        }

        let sub_keywords = sub_params.flat_keywords();
        let super_keywords = super_params.flat_keywords();
        for (keyword, super_type) in &super_keywords {
            match sub_keywords.iter().find(|(name, _)| name == keyword) {
                Some((_, sub_type)) => pairs.push(((*sub_type).clone(), (*super_type).clone())),
                None => match &sub_params.keywords.rest {
                    Some(sub_rest) => pairs.push((sub_rest.clone(), (*super_type).clone())),
                    None => return Err(failure()),
                },
            }
        }
        for (keyword, _) in &sub_params.keywords.requireds {
            if !super_params
                .keywords
                .requireds
                .iter()
                .any(|(name, _)| name == keyword)
            {
                return Err(failure());
            }
        }
        if let (Some(sub_rest), Some(super_rest)) =
            (&sub_params.keywords.rest, &super_params.keywords.rest)
        {
            pairs.push((sub_rest.clone(), super_rest.clone()));
        }
        Ok(pairs)
    }

    fn hole_path(&self, type_: &Type) -> Result<Option<Vec<Type>>, RbsError> {
        if let Type::Var(name) = type_ {
            return Ok(if self.is_unknown(name)? {
                Some(vec![type_.clone()])
            } else {
                None
            });
        }
        let mut shortest: Option<Vec<Type>> = None;
        for child in type_.each_child() {
            if let Some(mut path) = self.hole_path(child)? {
                path.insert(0, child.clone());
                if shortest
                    .as_ref()
                    .is_none_or(|shortest| path.len() < shortest.len())
                {
                    shortest = Some(path);
                }
            }
        }
        Ok(shortest)
    }
}
