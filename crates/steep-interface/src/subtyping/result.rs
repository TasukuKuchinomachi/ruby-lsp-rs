use rbs_definition::errors::RbsError;
use std::rc::Rc;

use crate::{
    subtyping::{
        check::abort_limit,
        constraints::UnsatisfiableConstraint,
        relation::{AnyRelation, Relation},
    },
    types::Type,
};

#[derive(Clone, Debug)]
pub enum FailureError {
    MethodMissing { name: String },
    BlockMismatch { name: String },
    ParameterMismatch { name: String },
    UnknownPair { relation: Relation<Type> },
    PolyMethodSubtyping { name: String },
    UnsatisfiedConstraints(Box<UnsatisfiableConstraint>),
    SelfBindingMismatch,
    LoopAbort,
}

impl FailureError {
    pub fn class_name(&self) -> &'static str {
        match self {
            FailureError::MethodMissing { .. } => "MethodMissingError",
            FailureError::BlockMismatch { .. } => "BlockMismatchError",
            FailureError::ParameterMismatch { .. } => "ParameterMismatchError",
            FailureError::UnknownPair { .. } => "UnknownPairError",
            FailureError::PolyMethodSubtyping { .. } => "PolyMethodSubtyping",
            FailureError::UnsatisfiedConstraints(_) => "UnsatisfiedConstraints",
            FailureError::SelfBindingMismatch => "SelfBindingMismatch",
            FailureError::LoopAbort => "LoopAbort",
        }
    }

    pub fn message(&self) -> String {
        match self {
            FailureError::MethodMissing { name } => format!("Method {name} is missing"),
            FailureError::BlockMismatch { name } => {
                format!("Method {name} is incompatible for block")
            }
            FailureError::ParameterMismatch { name } => {
                format!("Method {name} or its block has incompatible parameters")
            }
            FailureError::UnknownPair { relation } => format!("{relation} does not hold"),
            FailureError::PolyMethodSubtyping { name } => {
                format!("Method {name} requires uncheckable polymorphic subtyping")
            }
            FailureError::UnsatisfiedConstraints(error) => format!(
                "A constraint on {} cannot be solved: {} <: {}",
                error.variable, error.sub_type, error.super_type
            ),
            FailureError::SelfBindingMismatch => "Self binding is incompatible".to_string(),
            FailureError::LoopAbort => format!(
                "Detected infinite loop with limit of `{}`; specify $STEEP_SUBTYPING_ABORT_LIMIT env var to override the limit.",
                abort_limit()
            ),
        }
    }
}

#[derive(Debug)]
pub enum SubtypingResult {
    Skip(AnyRelation),
    Expand {
        relation: AnyRelation,
        child: Rc<SubtypingResult>,
    },
    All {
        relation: AnyRelation,
        branches: Vec<Rc<SubtypingResult>>,
    },
    Any {
        relation: AnyRelation,
        branches: Vec<Rc<SubtypingResult>>,
    },
    Success(AnyRelation),
    Failure {
        relation: AnyRelation,
        error: FailureError,
    },
}

impl SubtypingResult {
    pub fn relation(&self) -> &AnyRelation {
        match self {
            SubtypingResult::Skip(relation)
            | SubtypingResult::Expand { relation, .. }
            | SubtypingResult::All { relation, .. }
            | SubtypingResult::Any { relation, .. }
            | SubtypingResult::Success(relation)
            | SubtypingResult::Failure { relation, .. } => relation,
        }
    }

    pub fn class_name(&self) -> &'static str {
        match self {
            SubtypingResult::Skip(_) => "Skip",
            SubtypingResult::Expand { .. } => "Expand",
            SubtypingResult::All { .. } => "All",
            SubtypingResult::Any { .. } => "Any",
            SubtypingResult::Success(_) => "Success",
            SubtypingResult::Failure { .. } => "Failure",
        }
    }

    pub fn is_success(&self) -> bool {
        match self {
            SubtypingResult::Skip(_) | SubtypingResult::Failure { .. } => false,
            SubtypingResult::Success(_) => true,
            SubtypingResult::Expand { child, .. } => child.is_success(),
            SubtypingResult::All { branches, .. } => {
                !branches.iter().any(|branch| branch.is_failure())
            }
            SubtypingResult::Any { branches, .. } => {
                branches.iter().any(|branch| branch.is_success())
            }
        }
    }

    pub fn is_failure(&self) -> bool {
        match self {
            SubtypingResult::Skip(_) => false,
            _ => !self.is_success(),
        }
    }

    pub fn failure_path(&self) -> Vec<&SubtypingResult> {
        let mut path = Vec::new();
        self.collect_failure_path(&mut path);
        path
    }

    fn collect_failure_path<'a>(&'a self, path: &mut Vec<&'a SubtypingResult>) {
        match self {
            SubtypingResult::Skip(_) | SubtypingResult::Success(_) => {}
            SubtypingResult::Expand { child, .. } => {
                if child.is_failure() {
                    path.push(self);
                    child.collect_failure_path(path);
                }
            }
            SubtypingResult::All { branches, .. } => {
                if self.is_failure()
                    && let Some(branch) = branches.iter().find(|branch| branch.is_failure())
                {
                    path.push(self);
                    branch.collect_failure_path(path);
                }
            }
            SubtypingResult::Any { branches, .. } => {
                if self.is_failure() {
                    path.push(self);
                    if let Some(branch) = branches.iter().find(|branch| branch.is_failure()) {
                        branch.collect_failure_path(path);
                    }
                }
            }
            SubtypingResult::Failure { .. } => path.push(self),
        }
    }

    pub fn success(relation: impl Into<AnyRelation>) -> Rc<SubtypingResult> {
        Rc::new(SubtypingResult::Success(relation.into()))
    }

    pub fn failure(relation: impl Into<AnyRelation>, error: FailureError) -> Rc<SubtypingResult> {
        Rc::new(SubtypingResult::Failure {
            relation: relation.into(),
            error,
        })
    }

    pub fn expand(
        relation: impl Into<AnyRelation>,
        child: Rc<SubtypingResult>,
    ) -> Rc<SubtypingResult> {
        Rc::new(SubtypingResult::Expand {
            relation: relation.into(),
            child,
        })
    }
}

pub type Checked = Result<Rc<SubtypingResult>, RbsError>;

pub struct All {
    relation: AnyRelation,
    branches: Vec<Rc<SubtypingResult>>,
}

impl All {
    pub fn new(relation: impl Into<AnyRelation>) -> All {
        All {
            relation: relation.into(),
            branches: Vec::new(),
        }
    }

    pub fn is_success(&self) -> bool {
        !self.branches.iter().any(|branch| branch.is_failure())
    }

    pub fn add(
        &mut self,
        relation: impl Into<AnyRelation>,
        check: impl FnOnce() -> Checked,
    ) -> Result<bool, RbsError> {
        self.add_optional(relation, || check().map(Some))
    }

    pub fn add_optional(
        &mut self,
        relation: impl Into<AnyRelation>,
        check: impl FnOnce() -> Result<Option<Rc<SubtypingResult>>, RbsError>,
    ) -> Result<bool, RbsError> {
        if self.is_success() {
            if let Some(result) = check()? {
                self.branches.push(result);
            }
        } else {
            self.branches
                .push(Rc::new(SubtypingResult::Skip(relation.into())));
        }
        Ok(self.is_success())
    }

    pub fn add_result(&mut self, result: Option<Rc<SubtypingResult>>) -> bool {
        match result {
            Some(result) => {
                let relation = result.relation().clone();
                self.add(relation, || Ok(result))
                    .expect("adding a computed result does not fail")
            }
            None => self.is_success(),
        }
    }

    pub fn finish(self) -> Rc<SubtypingResult> {
        Rc::new(SubtypingResult::All {
            relation: self.relation,
            branches: self.branches,
        })
    }
}

pub struct Any {
    relation: AnyRelation,
    branches: Vec<Rc<SubtypingResult>>,
}

impl Any {
    pub fn new(relation: impl Into<AnyRelation>) -> Any {
        Any {
            relation: relation.into(),
            branches: Vec::new(),
        }
    }

    pub fn is_failure(&self) -> bool {
        !self.branches.iter().any(|branch| branch.is_success())
    }

    pub fn add(
        &mut self,
        relation: impl Into<AnyRelation>,
        check: impl FnOnce() -> Checked,
    ) -> Result<bool, RbsError> {
        if self.is_failure() {
            self.branches.push(check()?);
        } else {
            self.branches
                .push(Rc::new(SubtypingResult::Skip(relation.into())));
        }
        Ok(self.is_failure())
    }

    pub fn finish(self) -> Rc<SubtypingResult> {
        Rc::new(SubtypingResult::Any {
            relation: self.relation,
            branches: self.branches,
        })
    }
}
