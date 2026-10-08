use prism_child_nodes::child_nodes;
use rbs_environment::Type;
use ruby_prism::{Node, StatementsNode};

use super::{Checker, Context, Locals};
use crate::file_index::MAXIMUM_NODE_DEPTH;
use crate::source::{Span, constant_text};
use crate::type_utilities::{array_of, element_type, hash_of, is_bottom, remove_nil};

impl<'pr> Checker<'_, 'pr> {
    pub(crate) fn infer(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        if self.expression_depth >= MAXIMUM_NODE_DEPTH {
            return Type::Untyped;
        }
        self.expression_depth += 1;
        let result = self.infer_node(node, context, locals);
        self.expression_depth -= 1;
        result
    }

    pub(crate) fn infer_statements(
        &mut self,
        statements: &StatementsNode<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let mut last_type = Type::Nil;
        let mut diverged = false;
        for statement in statements.body().iter() {
            last_type = self.infer(&statement, context, locals);
            diverged |= is_bottom(&last_type);
        }
        if diverged { Type::Bottom } else { last_type }
    }

    pub(crate) fn infer_optional_statements(
        &mut self,
        statements: Option<StatementsNode<'pr>>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        match statements {
            Some(statements) => self.infer_statements(&statements, context, locals),
            None => Type::Nil,
        }
    }

    pub(crate) fn infer_optional_node(
        &mut self,
        node: Option<Node<'pr>>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        match node {
            Some(node) => self.infer(&node, context, locals),
            None => Type::Nil,
        }
    }

    fn infer_children(&mut self, node: &Node<'pr>, context: &Context, locals: &mut Locals) {
        for child in child_nodes(node) {
            self.infer(&child, context, locals);
        }
    }

    fn infer_node(&mut self, node: &Node<'pr>, context: &Context, locals: &mut Locals) -> Type {
        match node {
            Node::IntegerNode { .. } | Node::SourceLineNode { .. } => Type::instance("Integer"),
            Node::FloatNode { .. } => Type::instance("Float"),
            Node::RationalNode { .. } => Type::instance("Rational"),
            Node::ImaginaryNode { .. } => Type::instance("Complex"),
            Node::StringNode { .. } | Node::XStringNode { .. } | Node::SourceFileNode { .. } => {
                Type::instance("String")
            }
            Node::InterpolatedStringNode { .. } | Node::InterpolatedXStringNode { .. } => {
                self.infer_children(node, context, locals);
                Type::instance("String")
            }
            Node::SymbolNode { .. } => Type::instance("Symbol"),
            Node::InterpolatedSymbolNode { .. } => {
                self.infer_children(node, context, locals);
                Type::instance("Symbol")
            }
            Node::RegularExpressionNode { .. } => Type::instance("Regexp"),
            Node::InterpolatedRegularExpressionNode { .. } => {
                self.infer_children(node, context, locals);
                Type::instance("Regexp")
            }
            Node::MatchLastLineNode { .. }
            | Node::InterpolatedMatchLastLineNode { .. }
            | Node::FlipFlopNode { .. } => {
                self.infer_children(node, context, locals);
                Type::Bool
            }
            Node::SourceEncodingNode { .. } => Type::instance("Encoding"),
            Node::EmbeddedStatementsNode { .. } => {
                let Some(embedded) = node.as_embedded_statements_node() else {
                    return Type::Untyped;
                };
                self.infer_optional_statements(embedded.statements(), context, locals)
            }
            Node::EmbeddedVariableNode { .. } => {
                let Some(embedded) = node.as_embedded_variable_node() else {
                    return Type::Untyped;
                };
                self.infer(&embedded.variable(), context, locals)
            }
            Node::NilNode { .. } => Type::Nil,
            Node::TrueNode { .. } | Node::FalseNode { .. } => Type::Bool,
            Node::SelfNode { .. } => {
                self.record_type_hover(Span::of_node(node), &context.self_type);
                context.self_type.clone()
            }
            Node::ArrayNode { .. } => self.infer_array(node, context, locals),
            Node::HashNode { .. } | Node::KeywordHashNode { .. } => {
                self.infer_hash(node, context, locals)
            }
            Node::RangeNode { .. } => {
                let Some(range) = node.as_range_node() else {
                    return Type::Untyped;
                };
                let left = range.left().map(|left| self.infer(&left, context, locals));
                let right = range
                    .right()
                    .map(|right| self.infer(&right, context, locals));
                let bounds: Vec<Type> = left
                    .into_iter()
                    .chain(right)
                    .map(|bound| remove_nil(&bound))
                    .collect();
                let element = if bounds.is_empty() {
                    Type::Untyped
                } else {
                    Type::union(bounds)
                };
                Type::generic(
                    "Range",
                    vec![if is_bottom(&element) {
                        Type::Untyped
                    } else {
                        element
                    }],
                )
            }
            Node::SplatNode { .. } => {
                let Some(splat) = node.as_splat_node() else {
                    return Type::Untyped;
                };
                match splat.expression() {
                    Some(expression) => {
                        let splatted = self.infer(&expression, context, locals);
                        array_of(element_type(&splatted))
                    }
                    None => array_of(Type::Untyped),
                }
            }
            Node::ParenthesesNode { .. } => {
                let Some(parentheses) = node.as_parentheses_node() else {
                    return Type::Untyped;
                };
                self.infer_optional_node(parentheses.body(), context, locals)
            }
            Node::StatementsNode { .. } => {
                let Some(statements) = node.as_statements_node() else {
                    return Type::Untyped;
                };
                self.infer_statements(&statements, context, locals)
            }
            Node::LocalVariableReadNode { .. }
            | Node::LocalVariableWriteNode { .. }
            | Node::LocalVariableOrWriteNode { .. }
            | Node::LocalVariableAndWriteNode { .. }
            | Node::LocalVariableOperatorWriteNode { .. }
            | Node::ItLocalVariableReadNode { .. }
            | Node::InstanceVariableReadNode { .. }
            | Node::InstanceVariableWriteNode { .. }
            | Node::InstanceVariableOrWriteNode { .. }
            | Node::InstanceVariableAndWriteNode { .. }
            | Node::InstanceVariableOperatorWriteNode { .. }
            | Node::ClassVariableReadNode { .. }
            | Node::ClassVariableWriteNode { .. }
            | Node::ClassVariableOrWriteNode { .. }
            | Node::ClassVariableAndWriteNode { .. }
            | Node::ClassVariableOperatorWriteNode { .. }
            | Node::GlobalVariableReadNode { .. }
            | Node::GlobalVariableWriteNode { .. }
            | Node::GlobalVariableOrWriteNode { .. }
            | Node::GlobalVariableAndWriteNode { .. }
            | Node::GlobalVariableOperatorWriteNode { .. }
            | Node::MultiWriteNode { .. }
            | Node::MatchWriteNode { .. } => self.infer_variable(node, context, locals),
            // Steep types `$1` as `String?`, but it is almost always read right after a successful match, so `String`
            // avoids an error on nearly every use
            Node::BackReferenceReadNode { .. } | Node::NumberedReferenceReadNode { .. }
                if self.options.steep_compatible =>
            {
                Type::optional(Type::instance("String"))
            }
            Node::BackReferenceReadNode { .. } | Node::NumberedReferenceReadNode { .. } => {
                Type::instance("String")
            }
            Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. } => {
                self.infer_constant_reference(node, context, locals)
            }
            Node::ConstantWriteNode { .. }
            | Node::ConstantPathWriteNode { .. }
            | Node::ConstantOrWriteNode { .. }
            | Node::ConstantAndWriteNode { .. }
            | Node::ConstantOperatorWriteNode { .. }
            | Node::ConstantPathOrWriteNode { .. }
            | Node::ConstantPathAndWriteNode { .. }
            | Node::ConstantPathOperatorWriteNode { .. } => {
                self.infer_constant_write(node, context, locals)
            }
            Node::ShareableConstantNode { .. } => {
                let Some(shareable) = node.as_shareable_constant_node() else {
                    return Type::Untyped;
                };
                self.infer(&shareable.write(), context, locals)
            }
            Node::CallNode { .. } => {
                let Some(call) = node.as_call_node() else {
                    return Type::Untyped;
                };
                self.infer_call(&call, context, locals)
            }
            Node::CallOrWriteNode { .. }
            | Node::CallAndWriteNode { .. }
            | Node::CallOperatorWriteNode { .. }
            | Node::IndexOrWriteNode { .. }
            | Node::IndexAndWriteNode { .. }
            | Node::IndexOperatorWriteNode { .. } => self.infer_call_write(node, context, locals),
            Node::SuperNode { .. } | Node::ForwardingSuperNode { .. } => {
                self.infer_super(node, context, locals)
            }
            Node::YieldNode { .. } => self.infer_yield(node, context, locals),
            Node::IfNode { .. } | Node::UnlessNode { .. } => {
                self.infer_conditional(node, context, locals)
            }
            Node::CaseNode { .. } => self.infer_case(node, context, locals),
            Node::CaseMatchNode { .. } => self.infer_case_match(node, context, locals),
            Node::WhileNode { .. } | Node::UntilNode { .. } => {
                self.infer_loop(node, context, locals)
            }
            Node::ForNode { .. } => self.infer_for(node, context, locals),
            Node::AndNode { .. } => self.infer_and(node, context, locals),
            Node::OrNode { .. } => self.infer_or(node, context, locals),
            Node::BeginNode { .. } => self.infer_begin(node, context, locals),
            Node::RescueModifierNode { .. } => {
                let Some(rescue_modifier) = node.as_rescue_modifier_node() else {
                    return Type::Untyped;
                };
                let expression_type = self.infer(&rescue_modifier.expression(), context, locals);
                let rescue_type = self.infer(&rescue_modifier.rescue_expression(), context, locals);
                Type::union(vec![expression_type, rescue_type])
            }
            Node::ReturnNode { .. } => self.infer_return(node, context, locals),
            Node::NextNode { .. } => self.infer_next(node, context, locals),
            Node::BreakNode { .. } => self.infer_break(node, context, locals),
            Node::RetryNode { .. } | Node::RedoNode { .. } => Type::Bottom,
            Node::DefNode { .. } => self.infer_definition_node(node, context, locals),
            Node::ClassNode { .. } | Node::ModuleNode { .. } => {
                self.infer_class_node(node, context)
            }
            Node::SingletonClassNode { .. } => {
                self.infer_singleton_class_node(node, context, locals)
            }
            Node::LambdaNode { .. } => self.infer_lambda(node, context, locals),
            Node::DefinedNode { .. } => Type::optional(Type::instance("String")),
            Node::MatchPredicateNode { .. } | Node::MatchRequiredNode { .. } => {
                self.infer_pattern_match(node, context, locals)
            }
            Node::AliasMethodNode { .. }
            | Node::AliasGlobalVariableNode { .. }
            | Node::UndefNode { .. } => Type::Nil,
            Node::PreExecutionNode { .. } | Node::PostExecutionNode { .. } => {
                self.infer_children(node, context, locals);
                Type::Nil
            }
            Node::MissingNode { .. } => Type::Untyped,
            _ => {
                self.infer_children(node, context, locals);
                Type::Untyped
            }
        }
    }

    fn infer_array(&mut self, node: &Node<'pr>, context: &Context, locals: &mut Locals) -> Type {
        let Some(array) = node.as_array_node() else {
            return Type::Untyped;
        };
        let mut element_types = Vec::new();
        for element in array.elements().iter() {
            let element_type_value = self.infer(&element, context, locals);
            match element {
                Node::SplatNode { .. } => element_types.push(element_type(&element_type_value)),
                _ => element_types.push(element_type_value),
            }
        }
        if element_types.is_empty() {
            return array_of(Type::Untyped);
        }
        let element = Type::union(element_types);
        array_of(if is_bottom(&element) {
            Type::Untyped
        } else {
            element
        })
    }

    fn infer_hash(&mut self, node: &Node<'pr>, context: &Context, locals: &mut Locals) -> Type {
        let elements = match node {
            Node::HashNode { .. } => node.as_hash_node().map(|hash| hash.elements()),
            _ => node.as_keyword_hash_node().map(|hash| hash.elements()),
        };
        let Some(elements) = elements else {
            return Type::Untyped;
        };
        let mut key_types = Vec::new();
        let mut value_types = Vec::new();
        for element in elements.iter() {
            match element {
                Node::AssocNode { .. } => {
                    let Some(association) = element.as_assoc_node() else {
                        continue;
                    };
                    key_types.push(self.infer(&association.key(), context, locals));
                    let value = association.value();
                    if matches!(value, Node::ImplicitNode { .. }) {
                        value_types.push(self.infer_implicit_value(&value, context, locals));
                    } else {
                        value_types.push(self.infer(&value, context, locals));
                    }
                }
                Node::AssocSplatNode { .. } => {
                    let Some(splat) = element.as_assoc_splat_node() else {
                        continue;
                    };
                    let splatted = self.infer_optional_node(splat.value(), context, locals);
                    match splatted {
                        Type::ClassInstance {
                            ref name,
                            ref arguments,
                        } if name == "Hash" && arguments.len() == 2 => {
                            key_types.push(arguments[0].clone());
                            value_types.push(arguments[1].clone());
                        }
                        _ => {
                            key_types.push(Type::Untyped);
                            value_types.push(Type::Untyped);
                        }
                    }
                }
                _ => {
                    self.infer(&element, context, locals);
                }
            }
        }
        if key_types.is_empty() {
            return hash_of(Type::Untyped, Type::Untyped);
        }
        hash_of(Type::union(key_types), Type::union(value_types))
    }

    pub(crate) fn infer_implicit_value(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(implicit) = node.as_implicit_node() else {
            return Type::Untyped;
        };
        let value = implicit.value();
        match value {
            Node::LocalVariableReadNode { .. } => {
                let Some(read) = value.as_local_variable_read_node() else {
                    return Type::Untyped;
                };
                self.local_variable_type(&constant_text(&read.name()), context, locals)
            }
            _ => self
                .silently(|checker| checker.infer(&value, context, locals))
                .unwrap_or(Type::Untyped),
        }
    }
}
