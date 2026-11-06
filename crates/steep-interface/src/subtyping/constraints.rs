use std::{cell::RefCell, fmt, rc::Rc};

use rbs_definition::{collections::IndexMap, errors::RbsError};

use crate::{
    subtyping::{
        check::Check,
        relation::{AnyRelation, Relation},
        result::SubtypingResult,
        variable_variance::VariableVariance,
    },
    types::{RecordElement, Type},
};

#[derive(Clone, Debug)]
pub struct UnsatisfiableConstraint {
    pub variable: String,
    pub sub_type: Type,
    pub super_type: Type,
    pub result: Rc<SubtypingResult>,
}

#[derive(Clone, Debug, Default)]
struct Bounds {
    lower_bounds: Vec<Type>,
    upper_bounds: Vec<Type>,
    skips: Vec<Type>,
}

fn insert(set: &mut Vec<Type>, type_: Type) {
    if !set.contains(&type_) {
        set.push(type_);
    }
}

#[derive(Clone, Debug, Default)]
pub struct Constraints {
    dictionary: IndexMap<String, Bounds>,
    variables: Vec<String>,
}

pub type SharedConstraints = Rc<RefCell<Constraints>>;

impl Constraints {
    pub fn new(unknowns: &[String]) -> Constraints {
        let mut constraints = Constraints::default();
        for unknown in unknowns {
            constraints
                .dictionary
                .insert(unknown.clone(), Bounds::default());
        }
        constraints
    }

    pub fn empty() -> Constraints {
        Constraints::default()
    }

    pub fn shared(self) -> SharedConstraints {
        Rc::new(RefCell::new(self))
    }

    pub fn add(
        &mut self,
        variable: &str,
        sub_type: Option<&Type>,
        super_type: Option<&Type>,
        skip: bool,
    ) -> Result<(), RbsError> {
        let sub_type = sub_type.map(|type_| match type_ {
            Type::Logic(_) => Type::Boolean,
            type_ => type_.clone(),
        });
        let super_type = super_type.map(|type_| match type_ {
            Type::Logic(_) => Type::Boolean,
            type_ => type_.clone(),
        });
        let eliminated_super_type = super_type
            .as_ref()
            .filter(|type_| !matches!(type_, Type::Top))
            .map(|type_| self.eliminate_variable(type_, &Type::Top));
        let eliminated_sub_type = sub_type
            .as_ref()
            .filter(|type_| !matches!(type_, Type::Bot))
            .map(|type_| self.eliminate_variable(type_, &Type::Bot));
        let bounds = self
            .dictionary
            .get_mut(variable)
            .ok_or_else(|| format!("key not found: :{variable}"))?;
        if let Some(type_) = eliminated_super_type {
            insert(&mut bounds.upper_bounds, type_.clone());
            if skip {
                insert(&mut bounds.skips, type_);
            }
        }
        if let Some(type_) = eliminated_sub_type {
            insert(&mut bounds.lower_bounds, type_.clone());
            if skip {
                insert(&mut bounds.skips, type_);
            }
        }
        let free_variables = |types: &[Type]| -> Vec<String> {
            let mut variables = Vec::new();
            for type_ in types {
                type_.collect_free_variables(&mut variables);
            }
            variables
        };
        let super_free_variables = free_variables(&bounds.upper_bounds);
        let sub_free_variables = free_variables(&bounds.lower_bounds);
        let disjoint = |variables: &[String]| {
            !variables
                .iter()
                .any(|variable| self.dictionary.contains_key(variable))
        };
        if !(disjoint(&super_free_variables) || disjoint(&sub_free_variables)) {
            return Err(format!(
                "Invalid constraint: reason=Unknowns and FV(constraints) should be disjoint ({variable}, {}, {})",
                sub_type.map(|type_| type_.to_string()).unwrap_or_default(),
                super_type
                    .map(|type_| type_.to_string())
                    .unwrap_or_default()
            ).into());
        }
        Ok(())
    }

    fn eliminate_variable(&self, type_: &Type, to: &Type) -> Type {
        let any = |type_: &Type| self.eliminate_variable(type_, &Type::Any);
        match type_ {
            Type::NameInstance { name, args } => Type::NameInstance {
                name: name.clone(),
                args: args.iter().map(any).collect(),
            },
            Type::Alias { name, args } => Type::Alias {
                name: name.clone(),
                args: args.iter().map(any).collect(),
            },
            Type::Interface { name, args } => Type::Interface {
                name: name.clone(),
                args: args.iter().map(any).collect(),
            },
            Type::Union(types) => Type::union(types.iter().map(any).collect()),
            Type::Intersection(types) => Type::intersection(types.iter().map(any).collect()),
            Type::Var(name) => {
                if self.variables.contains(name) {
                    to.clone()
                } else {
                    type_.clone()
                }
            }
            Type::Tuple(types) => Type::Tuple(types.iter().map(any).collect()),
            Type::Record(elements) => Type::Record(
                elements
                    .iter()
                    .map(|element| RecordElement {
                        type_: any(&element.type_),
                        ..element.clone()
                    })
                    .collect(),
            ),
            Type::Proc { .. } => type_.map_type(&any),
            _ => type_.clone(),
        }
    }

    pub fn is_unknown(&self, variable: &str) -> bool {
        self.dictionary.contains_key(variable)
    }

    pub fn unknowns(&self) -> Vec<&String> {
        self.dictionary.keys().collect()
    }

    pub fn add_unknown(&mut self, variable: &str) {
        if !self.is_unknown(variable) {
            self.dictionary
                .insert(variable.to_string(), Bounds::default());
        }
    }

    pub fn add_variable(&mut self, variable: &str) -> Result<(), RbsError> {
        if !self.variables.iter().any(|existing| existing == variable) {
            self.variables.push(variable.to_string());
        }
        if self.is_unknown(variable) {
            return Err(format!(
                "Invalid constraint: reason=Variables and unknowns should be disjoint ([:{variable}])"
            ).into());
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.dictionary.is_empty()
    }

    fn bounds(&self, variable: &str) -> Result<&Bounds, RbsError> {
        self.dictionary
            .get(variable)
            .ok_or_else(|| format!("key not found: :{variable}").into())
    }

    pub fn upper_bound(&self, variable: &str, skip: bool) -> Result<Type, RbsError> {
        let upper_bounds = if skip {
            self.upper_bound_types(variable)?
        } else {
            self.bounds(variable)?.upper_bounds.clone()
        };
        Ok(match upper_bounds.len() {
            0 => Type::Top,
            1 => upper_bounds.into_iter().next().unwrap(),
            _ => Type::intersection(upper_bounds),
        })
    }

    pub fn lower_bound(&self, variable: &str) -> Result<Type, RbsError> {
        let lower_bounds = self.lower_bound_types(variable)?;
        Ok(match lower_bounds.len() {
            0 => Type::Bot,
            1 => lower_bounds.into_iter().next().unwrap(),
            _ => Type::union(lower_bounds),
        })
    }

    pub fn solution(
        &self,
        checker: &Check,
        variance: &VariableVariance,
        variables: &[String],
        self_type: Option<Type>,
        instance_type: Option<Type>,
        class_type: Option<Type>,
    ) -> Result<Result<Vec<(String, Type)>, UnsatisfiableConstraint>, RbsError> {
        let mut solution: Vec<(String, Type)> = Vec::new();
        for variable in self.dictionary.keys() {
            if !variables.contains(variable) {
                continue;
            }
            if !self.has_constraint(variable)? {
                solution.push((variable.clone(), Type::Any));
                continue;
            }
            let relation = Relation::new(
                self.lower_bound(variable)?,
                self.upper_bound(variable, false)?,
            );
            let result = checker.check(
                relation,
                Constraints::empty().shared(),
                self_type.clone(),
                instance_type.clone(),
                class_type.clone(),
            )?;
            if !result.is_success() {
                let AnyRelation::Type(relation) = result.relation() else {
                    unreachable!("the root of a check is a type relation");
                };
                return Ok(Err(UnsatisfiableConstraint {
                    variable: variable.clone(),
                    sub_type: relation.sub_type.clone(),
                    super_type: relation.super_type.clone(),
                    result: result.clone(),
                }));
            }
            let upper_bound = self.upper_bound(variable, true)?;
            let lower_bound = self.lower_bound(variable)?;
            let type_ = if variance.is_contravariant(variable) {
                upper_bound
            } else if variance.is_covariant(variable) {
                lower_bound
            } else if level_string(&lower_bound)? > level_string(&upper_bound)? {
                upper_bound
            } else {
                lower_bound
            };
            solution.push((variable.clone(), type_));
        }
        Ok(Ok(solution))
    }

    pub fn has_constraint(&self, variable: &str) -> Result<bool, RbsError> {
        Ok(!self.upper_bound_types(variable)?.is_empty()
            || !self.lower_bound_types(variable)?.is_empty())
    }

    pub fn lower_bound_types(&self, variable: &str) -> Result<Vec<Type>, RbsError> {
        Ok(self.bounds(variable)?.lower_bounds.clone())
    }

    pub fn upper_bound_types(&self, variable: &str) -> Result<Vec<Type>, RbsError> {
        let bounds = self.bounds(variable)?;
        Ok(if bounds.upper_bounds.is_empty() {
            bounds.skips.clone()
        } else if bounds.skips.is_empty() {
            bounds.upper_bounds.clone()
        } else {
            bounds
                .upper_bounds
                .iter()
                .filter(|type_| !bounds.skips.contains(type_))
                .cloned()
                .collect()
        })
    }

    pub fn each(&self) -> Vec<(String, Type, Type)> {
        self.dictionary
            .keys()
            .map(|variable| {
                (
                    variable.clone(),
                    self.lower_bound(variable)
                        .expect("a variable of the dictionary has bounds"),
                    self.upper_bound(variable, false)
                        .expect("a variable of the dictionary has bounds"),
                )
            })
            .collect()
    }
}

impl fmt::Display for Constraints {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let strings: Vec<String> = self
            .each()
            .into_iter()
            .map(|(variable, lower_bound, upper_bound)| {
                format!("{lower_bound} <: {variable} <: {upper_bound}")
            })
            .collect();
        let unknowns: Vec<&str> = self.dictionary.keys().map(String::as_str).collect();
        write!(
            formatter,
            "{}/{} |- {{ {} }}",
            unknowns.join(","),
            self.variables.join(","),
            strings.join(", ")
        )
    }
}

fn level_string(type_: &Type) -> Result<String, RbsError> {
    Ok(type_
        .level()?
        .iter()
        .map(ToString::to_string)
        .collect::<String>())
}
