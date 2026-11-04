use std::rc::Rc;

use steep_interface::{method_type::MethodType, substitution::Substitution, types::Type};

use crate::{
    ast::node::{
        type_application::{AppliedTypes, TypeApplication},
        type_assertion::AssertedType,
    },
    diagnostic::ruby as diagnostics,
    ruby::node::{Child, Node},
    type_construction::{Pair, TypeConstruction},
    type_inference::constant_env::nesting_names,
};

impl<'checker> TypeConstruction<'checker> {
    pub fn synthesize_assertion(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let asserted_node = node
            .child_node(0)
            .cloned()
            .expect("an assertion has an asserted node");
        let Child::TypeAssertion(as_type) = node.child(1) else {
            unreachable!("an assertion has a type assertion");
        };
        match as_type.type_(&nesting_names(&self.nesting()), self.checker) {
            AssertedType::Errors(errors) => {
                for error in &errors {
                    self.add_error(diagnostics::rbs_error(error));
                }
                self.synthesize(&asserted_node, hint, false)
            }
            AssertedType::Nothing | AssertedType::ParsingError(_) => {
                self.synthesize(&asserted_node, hint, false)
            }
            AssertedType::Type(type_) => {
                let pair = self.synthesize(&asserted_node, Some(&type_), false);
                let actual_type = pair.type_.clone();
                if self.no_subtyping(&type_, &actual_type).is_some()
                    && self.no_subtyping(&actual_type, &type_).is_some()
                {
                    self.add_error(diagnostics::false_assertion(node, &type_, &actual_type));
                }
                pair.constr.add_typing(node, type_)
            }
        }
    }

    pub fn synthesize_type_application(
        &self,
        node: &Rc<Node>,
        hint: Option<&Type>,
    ) -> Pair<'checker> {
        let sendish = node
            .child_node(0)
            .cloned()
            .expect("a type application has a sendish node");
        let Child::TypeApplication(tapp) = node.child(1) else {
            unreachable!("a tapp node has a type application");
        };
        if let AppliedTypes::Errors(errors) =
            tapp.types(&nesting_names(&self.nesting()), self.checker)
        {
            for error in &errors {
                self.add_error(diagnostics::rbs_error(error));
            }
        }
        let pair = self.synthesize_sendish(&sendish, hint, Some(tapp));
        pair.constr.add_typing(node, pair.type_)
    }

    pub fn apply_explicit_type_arguments(
        &self,
        method_type: &MethodType,
        tapp: &TypeApplication,
    ) -> Option<MethodType> {
        let type_args = tapp.types_or_nothing(&nesting_names(&self.nesting()), self.checker)?;
        let type_arity = method_type.type_params.len();
        let type_param_names: Vec<String> = method_type
            .type_params
            .iter()
            .map(|param| param.name.clone())
            .collect();
        if type_args.len() == type_arity {
            let mut arguments = Vec::new();
            for (type_, param) in type_args.iter().zip(&method_type.type_params) {
                match &param.upper_bound {
                    Some(upper_bound) => match self.no_subtyping(&type_.value, upper_bound) {
                        Some(result) => {
                            arguments.push(Type::Any);
                            self.add_error(diagnostics::type_argument_mismatch_error(
                                type_.location,
                                &type_.value,
                                param,
                                &result,
                            ));
                        }
                        None => arguments.push(type_.value.clone()),
                    },
                    None => arguments.push(type_.value.clone()),
                }
            }
            Some(method_type.instantiate(&Substitution::build(
                &type_param_names,
                &arguments,
                None,
                None,
                None,
            )))
        } else {
            if type_args.len() > type_arity {
                for type_arg in &type_args[type_arity..] {
                    self.add_error(diagnostics::unexpected_type_argument(
                        type_arg.location,
                        method_type,
                    ));
                }
            }
            if type_args.len() < type_arity {
                self.add_error(diagnostics::insufficient_type_argument(
                    &tapp.node(),
                    type_args.len(),
                    method_type,
                ));
            }
            Some(method_type.instantiate(&Substitution::build(
                &type_param_names,
                &vec![Type::Any; type_param_names.len()],
                None,
                None,
                None,
            )))
        }
    }
}
