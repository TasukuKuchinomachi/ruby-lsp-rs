use std::rc::Rc;

use steep_interface::{
    method_type::MethodType, subtyping::constraints::SharedConstraints, types::Type,
};

use crate::{
    annotation::Collection,
    ruby::node::Node,
    strict::StrictCheck,
    type_construction::{TypeConstruction, definition::arguments_node},
};

pub enum UntypedParametersSource {
    Annotation,
    MethodType,
    Overload,
}

fn send_node(node: &Rc<Node>) -> &Rc<Node> {
    match node.type_ {
        "block" | "numblock" | "itblock" => node.child_node(0).unwrap_or(node),
        _ => node,
    }
}

fn contains_untyped(type_: &Type) -> bool {
    matches!(type_, Type::Any) || type_.each_child().into_iter().any(contains_untyped)
}

fn quoted_list(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl<'checker> TypeConstruction<'checker> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn observe_unconstrained_type_variables(
        &self,
        node: &Rc<Node>,
        method_name: &str,
        declared_method_type: &MethodType,
        renamed_names: &[String],
        instantiated_method_type: &MethodType,
        return_type: &Type,
        constraints: &SharedConstraints,
        arguments: &[Rc<Node>],
        block_given: bool,
    ) {
        if arguments
            .iter()
            .filter(|argument| argument.type_ != "block_pass")
            .any(|argument| self.has_untyped_argument(argument))
        {
            return;
        }
        let block_given = block_given
            || arguments
                .iter()
                .any(|argument| argument.type_ == "block_pass");
        let block_return_variables = match &instantiated_method_type.block {
            Some(block) if block_given => block.type_.return_type.free_variables(),
            _ => Vec::new(),
        };
        let instantiated_variables = instantiated_method_type.type_.return_type.free_variables();
        let remaining_variables = return_type.free_variables();
        let constraints = constraints.borrow();
        let unconstrained: Vec<String> = renamed_names
            .iter()
            .zip(&declared_method_type.type_params)
            .filter(|(renamed, _)| {
                instantiated_variables.contains(renamed)
                    && !block_return_variables.contains(renamed)
                    && !remaining_variables.contains(renamed)
                    && constraints.is_unknown(renamed)
                    && !super::or_raise(constraints.has_constraint(renamed))
            })
            .map(|(_, declared)| declared.name.clone())
            .collect();
        if unconstrained.is_empty() {
            return;
        }
        let (noun, pronoun) = if unconstrained.len() == 1 {
            ("type variable", "it")
        } else {
            ("type variables", "them")
        };
        self.observe(
            StrictCheck::UnconstrainedTypeVariable,
            send_node(node),
            format!(
                "Steep types the {noun} {} in the return type of `{method_name}` as untyped because nothing constrains {pronoun}",
                quoted_list(&unconstrained)
            ),
        );
    }

    fn has_untyped_argument(&self, argument: &Rc<Node>) -> bool {
        match argument.type_ {
            "splat" | "kwsplat" => argument
                .child_node(0)
                .is_some_and(|child| self.has_untyped_argument(child)),
            "kwargs" => argument.child_nodes().any(|pair| match pair.type_ {
                "pair" => pair
                    .child_node(1)
                    .is_some_and(|value| self.has_untyped_argument(value)),
                _ => self.has_untyped_argument(pair),
            }),
            _ => self
                .typing
                .type_of(argument.id)
                .is_some_and(|type_| contains_untyped(&type_)),
        }
    }

    pub(super) fn observe_untyped_call_arguments(
        &self,
        node: &Rc<Node>,
        method_name: &str,
        arguments: &[Rc<Node>],
    ) {
        if arguments.is_empty() {
            return;
        }
        self.observe(
            StrictCheck::UntypedCallArguments,
            send_node(node),
            format!(
                "Steep does not check the arguments of `{method_name}` against its parameters because its method type takes `(?)`"
            ),
        );
    }

    pub(super) fn observe_untyped_method_parameters(
        &self,
        node: &Rc<Node>,
        method_name: &str,
        source: UntypedParametersSource,
        annotations: &Collection,
    ) {
        let Some(arguments) = arguments_node(node) else {
            return;
        };
        let reason = match source {
            UntypedParametersSource::Annotation => {
                format!("the `@type method` annotation of `{method_name}` takes `(?)`")
            }
            UntypedParametersSource::MethodType => {
                format!("the method type of `{method_name}` takes `(?)`")
            }
            UntypedParametersSource::Overload => {
                format!("an overload of `{method_name}` takes `(?)`")
            }
        };
        for argument in arguments.child_nodes() {
            match argument.type_ {
                "arg" | "optarg" | "kwarg" => {
                    let Some(name) = argument.child_symbol(0) else {
                        continue;
                    };
                    if annotations.var_types.contains_key(name) {
                        continue;
                    }
                    self.observe(
                        StrictCheck::UntypedMethodParameter,
                        argument,
                        format!("Steep types the parameter `{name}` as untyped because {reason}"),
                    );
                }
                "forward_arg" => {
                    self.observe(
                        StrictCheck::UntypedMethodParameter,
                        argument,
                        format!(
                            "Steep does not check the arguments forwarded with `...` because {reason}"
                        ),
                    );
                }
                _ => {}
            }
        }
    }

    pub(super) fn observe_untyped_yield_arguments(&self, node: &Rc<Node>, block_declared: bool) {
        let method_name = self
            .context
            .method_context
            .as_ref()
            .and_then(|context| context.name.clone())
            .unwrap_or_default();
        if block_declared {
            if node.child_nodes().next().is_none() {
                return;
            }
            self.observe(
                StrictCheck::UntypedYieldArguments,
                node,
                format!(
                    "Steep does not check the arguments of `yield` because the block type of `{method_name}` takes `(?)`"
                ),
            );
        } else {
            self.observe(
                StrictCheck::UntypedYieldArguments,
                node,
                format!(
                    "Steep types `yield` as untyped without checking its arguments because the method type of `{method_name}` takes `(?)` and declares no block"
                ),
            );
        }
    }
}
