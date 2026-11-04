use crate::{
    ast::{Callable, MethodType, Type, TypeName, TypeNameKind, TypeParameter, Variance},
    collections::IndexMap,
    environment::Environment,
    errors::RbsError,
    location::TypeLocation,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsedVariance {
    Unused,
    Covariant,
    Contravariant,
    Invariant,
}

pub struct VarianceResult {
    result: IndexMap<String, UsedVariance>,
}

impl VarianceResult {
    fn new(variables: &[String]) -> VarianceResult {
        VarianceResult {
            result: variables
                .iter()
                .map(|variable| (variable.clone(), UsedVariance::Unused))
                .collect(),
        }
    }

    fn covariant(&mut self, name: &str) {
        match self.result.get(name) {
            Some(UsedVariance::Unused) => {
                self.result
                    .insert(name.to_string(), UsedVariance::Covariant);
            }
            Some(UsedVariance::Contravariant) => {
                self.result
                    .insert(name.to_string(), UsedVariance::Invariant);
            }
            _ => {}
        }
    }

    fn contravariant(&mut self, name: &str) {
        match self.result.get(name) {
            Some(UsedVariance::Unused) => {
                self.result
                    .insert(name.to_string(), UsedVariance::Contravariant);
            }
            Some(UsedVariance::Covariant) => {
                self.result
                    .insert(name.to_string(), UsedVariance::Invariant);
            }
            _ => {}
        }
    }

    fn invariant(&mut self, name: &str) {
        self.result
            .insert(name.to_string(), UsedVariance::Invariant);
    }

    pub fn is_compatible(&self, name: &str, annotation: Variance) -> bool {
        let variance = self.result.get(name).copied();
        variance == Some(UsedVariance::Unused)
            || annotation == Variance::Invariant
            || matches!(
                (variance, annotation),
                (Some(UsedVariance::Covariant), Variance::Covariant)
                    | (Some(UsedVariance::Contravariant), Variance::Contravariant)
            )
    }

    pub fn incompatible(&self, parameters: &[TypeParameter]) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for parameter in parameters {
            if !self.is_compatible(&parameter.name, parameter.variance)
                && !names.contains(&parameter.name)
            {
                names.push(parameter.name.clone());
            }
        }
        names
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Covariant,
    Contravariant,
    Invariant,
}

fn negate(context: Context) -> Context {
    match context {
        Context::Invariant => Context::Invariant,
        Context::Covariant => Context::Contravariant,
        Context::Contravariant => Context::Covariant,
    }
}

pub struct VarianceCalculator<'environment> {
    environment: &'environment Environment,
}

impl<'environment> VarianceCalculator<'environment> {
    pub fn new(environment: &'environment Environment) -> VarianceCalculator<'environment> {
        VarianceCalculator { environment }
    }

    pub fn in_method_type(
        &self,
        method_type: &MethodType,
        variables: &[String],
    ) -> Result<VarianceResult, RbsError> {
        let mut result = VarianceResult::new(variables);
        self.function(&method_type.type_, &mut result, Context::Covariant)?;
        if let Some(block) = &method_type.block {
            self.function(&block.type_, &mut result, Context::Contravariant)?;
        }
        Ok(result)
    }

    pub fn in_inherit(
        &self,
        name: &TypeName,
        args: &[Type],
        variables: &[String],
    ) -> Result<VarianceResult, RbsError> {
        let type_ = if name.is_class() {
            Type::ClassInstance {
                name: name.clone(),
                args: args.to_vec(),
                location: TypeLocation::default(),
            }
        } else {
            Type::Interface {
                name: name.clone(),
                args: args.to_vec(),
                location: TypeLocation::default(),
            }
        };
        let mut result = VarianceResult::new(variables);
        self.type_(&type_, &mut result, Context::Covariant)?;
        Ok(result)
    }

    pub fn in_type_alias(&self, name: &TypeName) -> Result<VarianceResult, RbsError> {
        let entry = self
            .environment
            .type_alias_declarations
            .get(name)
            .ok_or_else(|| RbsError::RuntimeError(format!("Unknown alias name: {name}")))?;
        let variables: Vec<String> = entry
            .declaration
            .type_parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        let mut result = VarianceResult::new(&variables);
        self.type_(&entry.declaration.type_, &mut result, Context::Covariant)?;
        Ok(result)
    }

    fn type_(
        &self,
        type_: &Type,
        result: &mut VarianceResult,
        context: Context,
    ) -> Result<(), RbsError> {
        match type_ {
            Type::Variable(name) => {
                if result.result.contains_key(name) {
                    match context {
                        Context::Covariant => result.covariant(name),
                        Context::Contravariant => result.contravariant(name),
                        Context::Invariant => result.invariant(name),
                    }
                }
            }
            Type::ClassInstance { name, args, .. }
            | Type::Interface { name, args, .. }
            | Type::Alias { name, args, .. } => {
                let Some(type_name) = self.environment.normalize_type_name(name) else {
                    return Err(RbsError::NoTypeFound {
                        type_name: name.clone(),
                        location: type_.location().cloned(),
                    });
                };
                let parameters: &[TypeParameter] = match type_name.kind() {
                    TypeNameKind::Class => {
                        self.environment.class_declarations[&type_name].type_parameters()
                    }
                    TypeNameKind::Interface => {
                        &self.environment.interface_declarations[&type_name]
                            .declaration
                            .type_parameters
                    }
                    TypeNameKind::Alias => {
                        &self.environment.type_alias_declarations[&type_name]
                            .declaration
                            .type_parameters
                    }
                };
                for (arg, parameter) in args.iter().zip(parameters) {
                    let context = match parameter.variance {
                        Variance::Invariant => Context::Invariant,
                        Variance::Covariant => context,
                        Variance::Contravariant => negate(context),
                    };
                    self.type_(arg, result, context)?;
                }
            }
            Type::Proc { function, .. } => self.function(function, result, context)?,
            _ => {
                for child in type_.each_type() {
                    self.type_(child, result, context)?;
                }
            }
        }
        Ok(())
    }

    fn function(
        &self,
        callable: &Callable,
        result: &mut VarianceResult,
        context: Context,
    ) -> Result<(), RbsError> {
        for parameter in callable.each_parameter_type() {
            self.type_(parameter, result, negate(context))?;
        }
        self.type_(callable.return_type(), result, context)
    }
}
