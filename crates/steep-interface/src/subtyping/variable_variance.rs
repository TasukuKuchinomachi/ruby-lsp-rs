use crate::{
    method_type::{MethodType, Params},
    types::Type,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Variance {
    Covariant,
    Contravariant,
    Invariant,
}

#[derive(Clone, Debug, Default)]
pub struct VariableVariance {
    pub covariants: Vec<String>,
    pub contravariants: Vec<String>,
}

fn insert(set: &mut Vec<String>, name: &str) {
    if !set.iter().any(|existing| existing == name) {
        set.push(name.to_string());
    }
}

impl VariableVariance {
    pub fn is_covariant(&self, variable: &str) -> bool {
        self.covariants.iter().any(|name| name == variable)
            && !self.contravariants.iter().any(|name| name == variable)
    }

    pub fn is_contravariant(&self, variable: &str) -> bool {
        self.contravariants.iter().any(|name| name == variable)
            && !self.covariants.iter().any(|name| name == variable)
    }

    pub fn is_invariant(&self, variable: &str) -> bool {
        self.covariants.iter().any(|name| name == variable)
            && self.contravariants.iter().any(|name| name == variable)
    }

    pub fn from_type(type_: &Type) -> VariableVariance {
        let mut variance = VariableVariance::default();
        let VariableVariance {
            covariants,
            contravariants,
        } = &mut variance;
        add_type(type_, Variance::Covariant, covariants, contravariants);
        variance
    }

    pub fn from_method_type(method_type: &MethodType) -> VariableVariance {
        let mut variance = VariableVariance::default();
        let VariableVariance {
            covariants,
            contravariants,
        } = &mut variance;
        add_params(
            method_type.type_.params.as_ref(),
            false,
            covariants,
            contravariants,
        );
        add_type(
            &method_type.type_.return_type,
            Variance::Covariant,
            covariants,
            contravariants,
        );
        if let Some(block) = &method_type.block {
            add_params(
                block.type_.params.as_ref(),
                true,
                covariants,
                contravariants,
            );
            add_type(
                &block.type_.return_type,
                Variance::Contravariant,
                covariants,
                contravariants,
            );
        }
        variance
    }
}

fn add_params(
    params: Option<&Params>,
    block: bool,
    covariants: &mut Vec<String>,
    contravariants: &mut Vec<String>,
) {
    let variance = if block {
        Variance::Contravariant
    } else {
        Variance::Covariant
    };
    for type_ in params.into_iter().flat_map(Params::each_type) {
        add_type(type_, variance, covariants, contravariants);
    }
}

fn add_type(
    type_: &Type,
    variance: Variance,
    covariants: &mut Vec<String>,
    contravariants: &mut Vec<String>,
) {
    match type_ {
        Type::Var(name) => match variance {
            Variance::Covariant => insert(covariants, name),
            Variance::Contravariant => insert(contravariants, name),
            Variance::Invariant => {
                insert(covariants, name);
                insert(contravariants, name);
            }
        },
        Type::Proc {
            function, block, ..
        } => {
            for type_ in function.params.iter().flat_map(Params::each_type) {
                add_type(type_, variance, contravariants, covariants);
            }
            add_type(&function.return_type, variance, covariants, contravariants);
            if let Some(block) = block {
                for type_ in block.type_.params.iter().flat_map(Params::each_type) {
                    add_type(type_, variance, covariants, contravariants);
                }
                add_type(&function.return_type, variance, contravariants, covariants);
            }
        }
        Type::Union(types) | Type::Intersection(types) | Type::Tuple(types) => {
            for type_ in types {
                add_type(type_, variance, covariants, contravariants);
            }
        }
        Type::Interface { args, .. }
        | Type::NameInstance { args, .. }
        | Type::Alias { args, .. } => {
            for arg in args {
                add_type(arg, Variance::Invariant, covariants, contravariants);
            }
        }
        _ => {}
    }
}
