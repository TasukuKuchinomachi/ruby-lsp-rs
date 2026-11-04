use rbs_definition::errors::RbsError;
use std::{
    fmt,
    hash::{Hash, Hasher},
};

use rbs_definition::ast::Variance;

use crate::{
    substitution::Substitution,
    subtyping::{check::Check, constraints::Constraints, relation::Relation},
    types::{Type, fresh_variable_name, hash_of, unordered_hash},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PositionalKind {
    Required,
    Optional,
    Rest,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PositionalParam {
    pub kind: PositionalKind,
    pub type_: Type,
}

#[derive(Clone, Debug, Default)]
pub struct KeywordParams {
    pub requireds: Vec<(String, Type)>,
    pub optionals: Vec<(String, Type)>,
    pub rest: Option<Type>,
}

fn same_entries(left: &[(String, Type)], right: &[(String, Type)]) -> bool {
    left.len() == right.len()
        && left.iter().all(|(name, type_)| {
            right
                .iter()
                .any(|(other_name, other_type)| name == other_name && type_ == other_type)
        })
}

fn lookup<'a>(entries: &'a [(String, Type)], name: &str) -> Option<&'a Type> {
    entries
        .iter()
        .find(|(entry_name, _)| entry_name == name)
        .map(|(_, type_)| type_)
}

impl PartialEq for KeywordParams {
    fn eq(&self, other: &KeywordParams) -> bool {
        same_entries(&self.requireds, &other.requireds)
            && same_entries(&self.optionals, &other.optionals)
            && self.rest == other.rest
    }
}

impl Eq for KeywordParams {}

impl Hash for KeywordParams {
    fn hash<HashState: Hasher>(&self, state: &mut HashState) {
        unordered_hash(self.requireds.iter().map(hash_of), state);
        unordered_hash(self.optionals.iter().map(hash_of), state);
        self.rest.hash(state);
    }
}

impl KeywordParams {
    fn all_keys<'a>(&'a self, other: &'a KeywordParams) -> Vec<&'a String> {
        let mut keys: Vec<&String> = Vec::new();
        for (name, _) in self
            .requireds
            .iter()
            .chain(&self.optionals)
            .chain(&other.requireds)
            .chain(&other.optionals)
        {
            if !keys.contains(&name) {
                keys.push(name);
            }
        }
        keys
    }

    fn union(&self, other: &KeywordParams) -> KeywordParams {
        let mut requireds = Vec::new();
        let mut optionals = Vec::new();
        for key in self.all_keys(other) {
            let other_type = lookup(&other.requireds, key)
                .map(|type_| (true, type_))
                .or_else(|| lookup(&other.optionals, key).map(|type_| (false, type_)));
            if let Some(type_) = lookup(&self.requireds, key) {
                match other_type.or(other.rest.as_ref().map(|type_| (false, type_))) {
                    Some((true, other_type)) => requireds.push((
                        key.clone(),
                        Type::union(vec![type_.clone(), other_type.clone()]),
                    )),
                    Some((false, other_type)) => optionals.push((
                        key.clone(),
                        Type::union(vec![type_.clone(), other_type.clone()]),
                    )),
                    None => optionals.push((key.clone(), type_.clone())),
                }
            } else if let Some(type_) = lookup(&self.optionals, key) {
                match other_type.map(|(_, type_)| type_).or(other.rest.as_ref()) {
                    Some(other_type) => optionals.push((
                        key.clone(),
                        Type::union(vec![type_.clone(), other_type.clone()]),
                    )),
                    None => optionals.push((key.clone(), type_.clone())),
                }
            } else if let Some(type_) = &self.rest {
                if let Some((_, other_type)) = other_type {
                    optionals.push((
                        key.clone(),
                        Type::union(vec![type_.clone(), other_type.clone()]),
                    ));
                }
            } else if let Some((_, other_type)) = other_type {
                optionals.push((key.clone(), other_type.clone()));
            }
        }
        let rest = match (&self.rest, &other.rest) {
            (Some(left), Some(right)) => Some(Type::union(vec![left.clone(), right.clone()])),
            (left, right) => left.clone().or_else(|| right.clone()),
        };
        KeywordParams {
            requireds,
            optionals,
            rest,
        }
    }

    fn intersection(&self, other: &KeywordParams) -> Option<KeywordParams> {
        let mut requireds = Vec::new();
        let mut optionals = Vec::new();
        for key in self.all_keys(other) {
            let other_required = lookup(&other.requireds, key);
            let other_optional = lookup(&other.optionals, key);
            let intersection = |left: &Type, right: &Type| {
                (
                    key.clone(),
                    Type::intersection(vec![left.clone(), right.clone()]),
                )
            };
            if let Some(type_) = lookup(&self.requireds, key) {
                let other_type = other_required.or(other_optional).or(other.rest.as_ref())?;
                requireds.push(intersection(type_, other_type));
            } else if let Some(type_) = lookup(&self.optionals, key) {
                if let Some(other_type) = other_required {
                    requireds.push(intersection(type_, other_type));
                } else if let Some(other_type) = other_optional.or(other.rest.as_ref()) {
                    optionals.push(intersection(type_, other_type));
                }
            } else if let Some(type_) = &self.rest {
                if let Some(other_type) = other_required {
                    requireds.push(intersection(type_, other_type));
                } else if let Some(other_type) = other_optional {
                    optionals.push(intersection(type_, other_type));
                }
            } else if other_required.is_some() {
                return None;
            }
        }
        let rest = match (&self.rest, &other.rest) {
            (Some(left), Some(right)) => {
                Some(Type::intersection(vec![left.clone(), right.clone()]))
            }
            _ => None,
        };
        Some(KeywordParams {
            requireds,
            optionals,
            rest,
        })
    }

    fn map_type(&self, transform: &dyn Fn(&Type) -> Type) -> KeywordParams {
        let entries = |entries: &[(String, Type)]| -> Vec<(String, Type)> {
            entries
                .iter()
                .map(|(name, type_)| (name.clone(), transform(type_)))
                .collect()
        };
        KeywordParams {
            requireds: entries(&self.requireds),
            optionals: entries(&self.optionals),
            rest: self.rest.as_ref().map(transform),
        }
    }

    fn substitute(&self, substitution: &Substitution) -> KeywordParams {
        let entries = |entries: &[(String, Type)]| -> Vec<(String, Type)> {
            entries
                .iter()
                .map(|(name, type_)| (name.clone(), type_.substitute(substitution)))
                .collect()
        };
        KeywordParams {
            requireds: entries(&self.requireds),
            optionals: entries(&self.optionals),
            rest: self
                .rest
                .as_ref()
                .map(|type_| type_.substitute(substitution)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Params {
    pub positionals: Vec<PositionalParam>,
    pub keywords: KeywordParams,
}

fn positional(
    kind: PositionalKind,
    type_: Type,
    tail: Vec<PositionalParam>,
) -> Vec<PositionalParam> {
    let mut list = vec![PositionalParam { kind, type_ }];
    list.extend(tail);
    list
}

fn union_of(types: &[&Type], null: bool) -> Type {
    let mut types: Vec<Type> = types.iter().map(|type_| (*type_).clone()).collect();
    if null {
        types.push(Type::Nil);
    }
    Type::union(types)
}

fn merge_for_union(left: &[PositionalParam], right: &[PositionalParam]) -> Vec<PositionalParam> {
    use PositionalKind::{Optional, Required, Rest};
    match (left.first(), right.first()) {
        (None, None) => Vec::new(),
        (Some(x), Some(y)) => match (x.kind, y.kind) {
            (Required, Required) => positional(
                Required,
                union_of(&[&x.type_, &y.type_], false),
                merge_for_union(&left[1..], &right[1..]),
            ),
            (Required, Optional)
            | (Optional, Required)
            | (Optional, Optional)
            | (Optional, Rest) => positional(
                Optional,
                union_of(&[&x.type_, &y.type_], false),
                merge_for_union(&left[1..], &right[1..]),
            ),
            (Required, Rest) => positional(
                Optional,
                union_of(&[&x.type_, &y.type_], false),
                merge_for_union(&left[1..], right),
            ),
            (Rest, Required) | (Rest, Optional) => positional(
                Optional,
                union_of(&[&x.type_, &y.type_], false),
                merge_for_union(left, &right[1..]),
            ),
            (Rest, Rest) => vec![PositionalParam {
                kind: Rest,
                type_: union_of(&[&x.type_, &y.type_], false),
            }],
        },
        (Some(x), None) => match x.kind {
            Required => positional(Optional, x.type_.clone(), merge_for_union(&left[1..], &[])),
            Optional => positional(Optional, x.type_.clone(), merge_for_union(&left[1..], &[])),
            Rest => left.to_vec(),
        },
        (None, Some(y)) => match y.kind {
            Required => positional(Optional, y.type_.clone(), merge_for_union(&[], &right[1..])),
            Optional => positional(Optional, y.type_.clone(), merge_for_union(&[], &right[1..])),
            Rest => right.to_vec(),
        },
    }
}

fn merge_for_intersection(
    left: &[PositionalParam],
    right: &[PositionalParam],
) -> Option<Vec<PositionalParam>> {
    use PositionalKind::{Optional, Required, Rest};
    let intersection = |x: &PositionalParam, y: &PositionalParam| {
        Type::intersection(vec![x.type_.clone(), y.type_.clone()])
    };
    Some(match (left.first(), right.first()) {
        (None, None) => Vec::new(),
        (Some(x), Some(y)) => match (x.kind, y.kind) {
            (Required, Required) | (Required, Optional) | (Optional, Required) => positional(
                Required,
                intersection(x, y),
                merge_for_intersection(&left[1..], &right[1..])?,
            ),
            (Required, Rest) => positional(
                Required,
                intersection(x, y),
                merge_for_intersection(&left[1..], right)?,
            ),
            (Optional, Optional) => positional(
                Optional,
                intersection(x, y),
                merge_for_intersection(&left[1..], &right[1..])?,
            ),
            (Optional, Rest) => positional(
                Optional,
                intersection(x, y),
                merge_for_intersection(&left[1..], right)?,
            ),
            (Rest, Required) => positional(
                Required,
                intersection(x, y),
                merge_for_intersection(left, &right[1..])?,
            ),
            (Rest, Optional) => positional(
                Optional,
                intersection(x, y),
                merge_for_intersection(left, &right[1..])?,
            ),
            (Rest, Rest) => vec![PositionalParam {
                kind: Rest,
                type_: intersection(x, y),
            }],
        },
        (Some(x), None) => match x.kind {
            Required => return None,
            Optional | Rest => Vec::new(),
        },
        (None, Some(y)) => match y.kind {
            Required => return None,
            Optional | Rest => Vec::new(),
        },
    })
}

impl Params {
    pub fn build(
        required: Vec<Type>,
        optional: Vec<Type>,
        rest: Option<Type>,
        keywords: KeywordParams,
    ) -> Params {
        let mut positionals: Vec<PositionalParam> = Vec::new();
        positionals.extend(required.into_iter().map(|type_| PositionalParam {
            kind: PositionalKind::Required,
            type_,
        }));
        positionals.extend(optional.into_iter().map(|type_| PositionalParam {
            kind: PositionalKind::Optional,
            type_,
        }));
        positionals.extend(rest.map(|type_| PositionalParam {
            kind: PositionalKind::Rest,
            type_,
        }));
        Params {
            positionals,
            keywords,
        }
    }

    pub fn required(types: Vec<Type>) -> Params {
        Params::build(types, Vec::new(), None, KeywordParams::default())
    }

    pub fn required_types(&self) -> Vec<&Type> {
        self.positionals
            .iter()
            .take_while(|parameter| parameter.kind == PositionalKind::Required)
            .map(|parameter| &parameter.type_)
            .collect()
    }

    pub fn optional_types(&self) -> Vec<&Type> {
        self.positionals
            .iter()
            .take_while(|parameter| parameter.kind != PositionalKind::Rest)
            .filter(|parameter| parameter.kind == PositionalKind::Optional)
            .map(|parameter| &parameter.type_)
            .collect()
    }

    pub fn flat_unnamed_params(&self) -> Vec<(PositionalKind, &Type)> {
        self.positionals
            .iter()
            .filter(|parameter| parameter.kind != PositionalKind::Rest)
            .map(|parameter| (parameter.kind, &parameter.type_))
            .collect()
    }

    pub fn flat_keywords(&self) -> Vec<(&String, &Type)> {
        let mut keywords: Vec<(&String, &Type)> = self
            .keywords
            .requireds
            .iter()
            .map(|(name, type_)| (name, type_))
            .collect();
        for (name, type_) in &self.keywords.optionals {
            match keywords.iter_mut().find(|(existing, _)| *existing == name) {
                Some(entry) => entry.1 = type_,
                None => keywords.push((name, type_)),
            }
        }
        keywords
    }

    pub fn rest_type(&self) -> Option<&Type> {
        self.positionals
            .iter()
            .skip_while(|parameter| parameter.kind != PositionalKind::Rest)
            .map(|parameter| &parameter.type_)
            .next()
    }

    pub fn union(&self, other: &Params) -> Params {
        Params {
            positionals: merge_for_union(&self.positionals, &other.positionals),
            keywords: self.keywords.union(&other.keywords),
        }
    }

    pub fn intersection(&self, other: &Params) -> Option<Params> {
        Some(Params {
            positionals: merge_for_intersection(&self.positionals, &other.positionals)?,
            keywords: self.keywords.intersection(&other.keywords)?,
        })
    }

    fn substitute(&self, substitution: &Substitution) -> Params {
        Params {
            positionals: self
                .positionals
                .iter()
                .map(|parameter| PositionalParam {
                    kind: parameter.kind,
                    type_: parameter.type_.substitute(substitution),
                })
                .collect(),
            keywords: self.keywords.substitute(substitution),
        }
    }

    pub fn map_type(&self, transform: &dyn Fn(&Type) -> Type) -> Params {
        Params {
            positionals: self
                .positionals
                .iter()
                .map(|parameter| PositionalParam {
                    kind: parameter.kind,
                    type_: transform(&parameter.type_),
                })
                .collect(),
            keywords: self.keywords.map_type(transform),
        }
    }

    pub fn each_type(&self) -> impl Iterator<Item = &Type> {
        self.positionals
            .iter()
            .map(|parameter| &parameter.type_)
            .chain(self.keywords.requireds.iter().map(|(_, type_)| type_))
            .chain(self.keywords.optionals.iter().map(|(_, type_)| type_))
            .chain(self.keywords.rest.iter())
    }
}

impl fmt::Display for Params {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parameters: Vec<String> = Vec::new();
        parameters.extend(self.required_types().iter().map(ToString::to_string));
        parameters.extend(
            self.optional_types()
                .iter()
                .map(|type_| format!("?{type_}")),
        );
        if let Some(type_) = self.rest_type() {
            parameters.push(format!("*{type_}"));
        }
        parameters.extend(
            self.keywords
                .requireds
                .iter()
                .map(|(name, type_)| format!("{name}: {type_}")),
        );
        parameters.extend(
            self.keywords
                .optionals
                .iter()
                .map(|(name, type_)| format!("?{name}: {type_}")),
        );
        if let Some(type_) = &self.keywords.rest {
            parameters.push(format!("**{type_}"));
        }
        write!(formatter, "({})", parameters.join(", "))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Function {
    pub params: Option<Params>,
    pub return_type: Type,
}

impl fmt::Display for Function {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.params {
            Some(params) => write!(formatter, "{params} -> {}", self.return_type),
            None => write!(formatter, "(?) -> {}", self.return_type),
        }
    }
}

impl Function {
    pub fn each_type(&self) -> Vec<&Type> {
        let mut types: Vec<&Type> = self
            .params
            .iter()
            .flat_map(|params| params.each_type())
            .collect();
        types.push(&self.return_type);
        types
    }

    pub(crate) fn collect_free_variables(&self, variables: &mut Vec<String>) {
        for type_ in self.each_type() {
            type_.collect_free_variables(variables);
        }
    }

    pub fn map_type(&self, transform: &dyn Fn(&Type) -> Type) -> Function {
        Function {
            params: self
                .params
                .as_ref()
                .map(|params| params.map_type(transform)),
            return_type: transform(&self.return_type),
        }
    }

    pub fn substitute(&self, substitution: &Substitution) -> Function {
        Function {
            params: self
                .params
                .as_ref()
                .map(|params| params.substitute(substitution)),
            return_type: self.return_type.substitute(substitution),
        }
    }

    pub fn parameters_to_string(&self) -> String {
        self.params
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    pub fn with_return_type(&self, return_type: Type) -> Function {
        Function {
            params: self.params.clone(),
            return_type,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Block {
    pub type_: Function,
    pub optional: bool,
    pub self_type: Option<Type>,
}

impl Block {
    pub(crate) fn collect_free_variables(&self, variables: &mut Vec<String>) {
        self.type_.collect_free_variables(variables);
        if let Some(self_type) = &self.self_type {
            self_type.collect_free_variables(variables);
        }
    }

    pub fn map_type(&self, transform: &dyn Fn(&Type) -> Type) -> Block {
        Block {
            type_: self.type_.map_type(transform),
            optional: self.optional,
            self_type: self
                .self_type
                .as_ref()
                .map(|self_type| self_type.map_type(transform)),
        }
    }

    pub fn substitute(&self, substitution: &Substitution) -> Block {
        Block {
            type_: self.type_.substitute(substitution),
            optional: self.optional,
            self_type: self
                .self_type
                .as_ref()
                .map(|self_type| self_type.substitute(substitution)),
        }
    }
}

impl fmt::Display for Block {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let self_binding = self
            .self_type
            .as_ref()
            .map(|self_type| format!("[self: {self_type}] "))
            .unwrap_or_default();
        write!(
            formatter,
            "{}{{ {} {self_binding}-> {} }}",
            if self.optional { "?" } else { "" },
            self.type_.parameters_to_string(),
            self.type_.return_type
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeParam {
    pub name: String,
    pub upper_bound: Option<Type>,
    pub variance: Variance,
    pub unchecked: bool,
    pub default_type: Option<Type>,
}

impl fmt::Display for TypeParam {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.unchecked {
            formatter.write_str("unchecked ")?;
        }
        match self.variance {
            Variance::Invariant => {}
            Variance::Covariant => formatter.write_str("out ")?,
            Variance::Contravariant => formatter.write_str("in ")?,
        }
        formatter.write_str(&self.name)?;
        if let Some(upper_bound) = &self.upper_bound {
            write!(formatter, " < {upper_bound}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MethodType {
    pub type_params: Vec<TypeParam>,
    pub type_: Function,
    pub block: Option<Block>,
}

impl MethodType {
    pub fn substitute(&self, substitution: &Substitution) -> MethodType {
        let names: Vec<String> = self
            .type_params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        let substitution = substitution.except(&names);
        MethodType {
            type_params: self.type_params.clone(),
            type_: self.type_.substitute(&substitution),
            block: self
                .block
                .as_ref()
                .map(|block| block.substitute(&substitution)),
        }
    }

    pub fn with_return_type(&self, return_type: Type) -> MethodType {
        MethodType {
            type_: self.type_.with_return_type(return_type),
            ..self.clone()
        }
    }

    pub fn with_type_params(&self, type_params: Vec<TypeParam>) -> MethodType {
        MethodType {
            type_params,
            ..self.clone()
        }
    }

    pub fn instantiate(&self, substitution: &Substitution) -> MethodType {
        MethodType {
            type_params: Vec::new(),
            type_: self.type_.substitute(substitution),
            block: self
                .block
                .as_ref()
                .map(|block| block.substitute(substitution)),
        }
    }

    fn renamed_type_params(&self, new_names: &[String]) -> (Vec<TypeParam>, Substitution) {
        let old_names: Vec<String> = self
            .type_params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        let new_types: Vec<Type> = new_names.iter().cloned().map(Type::Var).collect();
        let substitution = Substitution::build(&old_names, &new_types, None, None, None);
        let parameters = self
            .type_params
            .iter()
            .zip(new_names)
            .map(|(parameter, new_name)| TypeParam {
                name: new_name.clone(),
                upper_bound: parameter
                    .upper_bound
                    .as_ref()
                    .map(|type_| type_.substitute(&substitution)),
                variance: parameter.variance,
                unchecked: parameter.unchecked,
                default_type: parameter
                    .default_type
                    .as_ref()
                    .map(|type_| type_.substitute(&substitution)),
            })
            .collect();
        (parameters, substitution)
    }

    fn equals_modulo_type_params(&self, other: &MethodType) -> bool {
        if self.type_params.is_empty() && other.type_params.is_empty() {
            return self == other;
        }
        if self.type_params.len() != other.type_params.len() {
            return false;
        }
        let new_names: Vec<String> = self
            .type_params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        let (self_parameters, self_substitution) = self.renamed_type_params(&new_names);
        let (other_parameters, other_substitution) = other.renamed_type_params(&new_names);
        self_parameters == other_parameters
            && self.instantiate(&self_substitution) == other.instantiate(&other_substitution)
    }

    fn union_without_type_params(&self, other: &MethodType) -> Option<MethodType> {
        if self == other {
            return Some(self.clone());
        }
        let params = match (&self.type_.params, &other.type_.params) {
            (Some(left), Some(right)) => left.intersection(right)?,
            _ => return None,
        };
        let block = match (&self.block, &other.block) {
            (Some(left), Some(right)) => {
                let self_type = match (&left.self_type, &right.self_type) {
                    (Some(left), Some(right)) => {
                        Some(Type::union(vec![left.clone(), right.clone()]))
                    }
                    (Some(_), None) | (None, Some(_)) => Some(Type::Bot),
                    (None, None) => None,
                };
                let block_params = match (&left.type_.params, &right.type_.params) {
                    (Some(left_params), Some(right_params)) => {
                        left_params.intersection(right_params)?;
                        Some(left_params.union(right_params))
                    }
                    (left_params, right_params) => {
                        left_params.clone().or_else(|| right_params.clone())
                    }
                };
                Some(Block {
                    type_: Function {
                        params: block_params,
                        return_type: Type::intersection(vec![
                            left.type_.return_type.clone(),
                            right.type_.return_type.clone(),
                        ]),
                    },
                    optional: left.optional && right.optional,
                    self_type,
                })
            }
            (Some(left), None) if left.optional => Some(left.clone()),
            (_, Some(right)) if right.optional => Some(right.clone()),
            (None, None) => None,
            _ => return None,
        };
        Some(MethodType {
            type_params: Vec::new(),
            type_: Function {
                params: Some(params),
                return_type: Type::union(vec![
                    self.type_.return_type.clone(),
                    other.type_.return_type.clone(),
                ]),
            },
            block,
        })
    }

    fn type_param_names(&self) -> Vec<String> {
        self.type_params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect()
    }

    fn fresh_type_params(&self) -> (Vec<TypeParam>, Substitution) {
        let new_names: Vec<String> = self
            .type_params
            .iter()
            .map(|parameter| fresh_variable_name(&parameter.name))
            .collect();
        self.renamed_type_params(&new_names)
    }

    pub fn union(
        type1: &MethodType,
        type2: &MethodType,
        check: &Check,
    ) -> Result<Option<MethodType>, RbsError> {
        Self::try_type_params(
            type1,
            type2,
            check,
            MethodType::union_without_type_params,
            Relation::new,
        )
    }

    fn try_type_params(
        type1: &MethodType,
        type2: &MethodType,
        check: &Check,
        generate: fn(&MethodType, &MethodType) -> Option<MethodType>,
        relation: fn(MethodType, MethodType) -> Relation<MethodType>,
    ) -> Result<Option<MethodType>, RbsError> {
        if type1.equals_modulo_type_params(type2) {
            return Ok(Some(type1.clone()));
        }
        match (type1.type_params.is_empty(), type2.type_params.is_empty()) {
            (true, true) => Ok(generate(type1, type2)),
            (true, false) => Ok(generate(type1, &type2.with_type_params(Vec::new()))
                .map(|method_type| method_type.with_type_params(type2.type_params.clone()))),
            (false, true) => Ok(generate(&type1.with_type_params(Vec::new()), type2)
                .map(|method_type| method_type.with_type_params(type1.type_params.clone()))),
            (false, false) if type1.type_params.len() == type2.type_params.len() => {
                let (parameters1, substitution1) = type1.fresh_type_params();
                let (parameters2, substitution2) = type2.fresh_type_params();
                let Some(generated) = generate(
                    &type1.instantiate(&substitution1),
                    &type2.instantiate(&substitution2),
                ) else {
                    return Ok(None);
                };
                let bounds = parameters1
                    .iter()
                    .chain(&parameters2)
                    .map(|parameter| (parameter.name.clone(), parameter.upper_bound.clone()))
                    .collect();
                check.push_variable_bounds(bounds, || {
                    let names1 = type1.type_param_names();
                    let names2 = type2.type_param_names();
                    let variables: Vec<String> = names1.iter().chain(&names2).cloned().collect();
                    let constraints = Constraints::new(&variables).shared();
                    check.with_context(
                        Some(Type::Any),
                        Some(Type::Any),
                        Some(Type::Any),
                        constraints.clone(),
                        || {
                            let result1 = check.check_method_type(
                                "__method_on_type1",
                                relation(type1.with_type_params(Vec::new()), generated.clone()),
                            )?;
                            let result2 = check.check_method_type(
                                "__method_on_type2",
                                relation(type2.with_type_params(Vec::new()), generated.clone()),
                            )?;
                            if !(result1.is_success() && result2.is_success()) {
                                return Ok(None);
                            }
                            let constraints = constraints.borrow();
                            for (name1, name2) in names1.iter().zip(&names2) {
                                let same = constraints.upper_bound(name1, false)?
                                    == constraints.upper_bound(name2, false)?
                                    && constraints.lower_bound(name1)?
                                        == constraints.lower_bound(name2)?;
                                if !same {
                                    return Ok(None);
                                }
                            }
                            let (renamed_parameters2, renamed_substitution2) =
                                type2.renamed_type_params(&names1);
                            let Some(method_type) = generate(
                                &type1.with_type_params(Vec::new()),
                                &type2.instantiate(&renamed_substitution2),
                            ) else {
                                return Ok(None);
                            };
                            let type_params = type1
                                .type_params
                                .iter()
                                .zip(&renamed_parameters2)
                                .map(|(parameter1, parameter2)| {
                                    match (&parameter1.upper_bound, &parameter2.upper_bound) {
                                        (Some(bound1), Some(bound2)) => TypeParam {
                                            upper_bound: Some(Type::union(vec![
                                                bound1.clone(),
                                                bound2.clone(),
                                            ])),
                                            ..parameter1.clone()
                                        },
                                        (_, Some(bound2)) => TypeParam {
                                            upper_bound: Some(bound2.clone()),
                                            ..parameter1.clone()
                                        },
                                        _ => parameter1.clone(),
                                    }
                                })
                                .collect();
                            Ok(Some(method_type.with_type_params(type_params)))
                        },
                    )
                })
            }
            (false, false) => Ok(None),
        }
    }
}

impl fmt::Display for MethodType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.type_params.is_empty() {
            let parameters: Vec<String> =
                self.type_params.iter().map(ToString::to_string).collect();
            write!(formatter, "[{}] ", parameters.join(", "))?;
        }
        formatter.write_str(&self.type_.parameters_to_string())?;
        if let Some(block) = &self.block {
            write!(formatter, " {block}")?;
        }
        write!(formatter, " -> {}", self.type_.return_type)
    }
}
