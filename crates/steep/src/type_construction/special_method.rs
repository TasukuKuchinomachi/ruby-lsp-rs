use std::rc::Rc;

use steep_interface::{builder::Overload, types::Type};

use crate::{
    ruby::node::Node,
    ruby_exception,
    type_construction::{TypeConstruction, instance_type, is_instance_of},
    type_inference::method_call::{MethodCall, MethodCallKind},
};

const ARRAY_COMPACT: &[&str] = &["::Array#compact", "::Enumerable#compact"];
const HASH_COMPACT: &[&str] = &["::Hash#compact"];
const LAMBDA: &[&str] = &["::Kernel#lambda", "::Kernel.lambda"];

impl<'checker> TypeConstruction<'checker> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_special_method(
        &self,
        node: &Rc<Node>,
        receiver_type: &Type,
        method_name: &str,
        overload: &Overload,
        arguments: &[Rc<Node>],
        block_params: Option<&Rc<Node>>,
        block_body: Option<&Rc<Node>>,
        hint: Option<&Type>,
    ) -> Option<(MethodCall, TypeConstruction<'checker>)> {
        let method_type = &overload.method_type;
        let declarations =
            super::send::unique(super::send::method_declarations(method_name, overload));
        let find = |names: &[&str]| {
            declarations
                .iter()
                .find(|declaration| names.contains(&declaration.full_name().as_str()))
                .cloned()
        };
        let special_call = |declaration_method_name: &str, type_: Type| MethodCall {
            node: node.clone(),
            method_name: declaration_method_name.to_string(),
            receiver_type: receiver_type.clone(),
            return_type: type_.clone(),
            method_decls: declarations.clone(),
            kind: MethodCallKind::Special {
                actual_method_type: method_type.with_return_type(type_),
            },
        };
        if let Some(declaration) = find(ARRAY_COMPACT) {
            if arguments.is_empty() && block_params.is_none() {
                let return_type = &method_type.type_.return_type;
                if is_instance_of(return_type, "::Array")
                    && let Type::NameInstance { args, .. } = return_type
                {
                    let element = ruby_exception::fetch(args, 0);
                    let type_ = instance_type("::Array", vec![self.unwrap(element)]);
                    let constr = self.add_typing(node, type_.clone()).constr;
                    return Some((special_call(&declaration.method_name, type_), constr));
                }
            }
        } else if let Some(declaration) = find(HASH_COMPACT) {
            if arguments.is_empty() && block_params.is_none() {
                let return_type = &method_type.type_.return_type;
                if is_instance_of(return_type, "::Hash")
                    && let Type::NameInstance { args, .. } = return_type
                {
                    let key = ruby_exception::fetch(args, 0).clone();
                    let value = ruby_exception::fetch(args, 1);
                    let type_ = instance_type("::Hash", vec![key, self.unwrap(value)]);
                    let constr = self.add_typing(node, type_.clone()).constr;
                    return Some((special_call(&declaration.method_name, type_), constr));
                }
            }
        } else if let Some(declaration) = find(LAMBDA)
            && let Some(block_params) = block_params
        {
            let pair = self.type_lambda(node, block_params, block_body, hint);
            return Some((
                special_call(&declaration.method_name, pair.type_),
                pair.constr,
            ));
        }
        None
    }
}
