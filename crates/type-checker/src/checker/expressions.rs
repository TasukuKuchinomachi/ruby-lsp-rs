use prism_child_nodes::child_nodes;
use rbs_environment::{Literal, Type};
use ruby_prism::{Node, StatementsNode};

use super::{Checker, Context, Locals};
use crate::file_index::MAXIMUM_NODE_DEPTH;
use crate::source::{Span, bytes_text, constant_text};
use crate::type_utilities::{array_of, element_type, is_bottom, remove_nil};

impl<'pr> Checker<'_, 'pr> {
    pub(crate) fn infer(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        self.infer_with_hint(node, None, context, locals)
    }

    /// Steep's `synthesize(node, hint:)`: the hint is the type the context expects of the node, which gives a literal
    /// its literal, tuple or record type and a generic call its return type
    pub(crate) fn infer_with_hint(
        &mut self,
        node: &Node<'pr>,
        hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        if self.expression_depth >= MAXIMUM_NODE_DEPTH {
            return Type::Untyped;
        }
        self.expression_depth += 1;
        let result = self.infer_node(node, hint, context, locals);
        self.expression_depth -= 1;
        result
    }

    /// Only the last statement gets the hint, as it is the value of the statements
    pub(crate) fn infer_statements(
        &mut self,
        statements: &StatementsNode<'pr>,
        hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let body: Vec<Node<'pr>> = statements.body().iter().collect();
        let mut last_type = Type::Nil;
        let mut diverged = false;
        for (index, statement) in body.iter().enumerate() {
            let statement_hint = if index + 1 == body.len() { hint } else { None };
            last_type = self.infer_with_hint(statement, statement_hint, context, locals);
            diverged |= is_bottom(&last_type);
        }
        if diverged { Type::Bottom } else { last_type }
    }

    pub(crate) fn infer_optional_statements(
        &mut self,
        statements: Option<StatementsNode<'pr>>,
        hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        match statements {
            Some(statements) => self.infer_statements(&statements, hint, context, locals),
            None => Type::Nil,
        }
    }

    pub(crate) fn infer_optional_node(
        &mut self,
        node: Option<Node<'pr>>,
        hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        match node {
            Some(node) => self.infer_with_hint(&node, hint, context, locals),
            None => Type::Nil,
        }
    }

    fn infer_children(&mut self, node: &Node<'pr>, context: &Context, locals: &mut Locals) {
        for child in child_nodes(node) {
            self.infer(&child, context, locals);
        }
    }

    fn infer_node(&mut self, node: &Node<'pr>, hint: Option<&Type>, context: &Context, locals: &mut Locals) -> Type {
        match node {
            Node::IntegerNode { .. } => {
                let text = crate::source::bytes_text(node.location().as_slice()).replace('_', "");
                self.literal_type_from_hint(Literal::Integer(text), hint)
                    .unwrap_or_else(|| Type::instance("Integer"))
            }
            Node::SourceLineNode { .. } => Type::instance("Integer"),
            Node::FloatNode { .. } => Type::instance("Float"),
            Node::RationalNode { .. } => Type::instance("Rational"),
            Node::ImaginaryNode { .. } => Type::instance("Complex"),
            Node::StringNode { .. } => node
                .as_string_node()
                .and_then(|string| self.literal_type_from_hint(Literal::String(bytes_text(string.unescaped())), hint))
                .unwrap_or_else(|| Type::instance("String")),
            Node::XStringNode { .. } | Node::SourceFileNode { .. } => Type::instance("String"),
            Node::InterpolatedStringNode { .. } | Node::InterpolatedXStringNode { .. } => {
                self.infer_children(node, context, locals);
                Type::instance("String")
            }
            Node::SymbolNode { .. } => node
                .as_symbol_node()
                .and_then(|symbol| self.literal_type_from_hint(Literal::Symbol(bytes_text(symbol.unescaped())), hint))
                .unwrap_or_else(|| Type::instance("Symbol")),
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
                self.infer_optional_statements(embedded.statements(), None, context, locals)
            }
            Node::EmbeddedVariableNode { .. } => {
                let Some(embedded) = node.as_embedded_variable_node() else {
                    return Type::Untyped;
                };
                self.infer(&embedded.variable(), context, locals)
            }
            Node::NilNode { .. } => Type::Nil,
            Node::TrueNode { .. } => self.boolean_literal_type(Literal::True, hint),
            Node::FalseNode { .. } => self.boolean_literal_type(Literal::False, hint),
            Node::SelfNode { .. } => {
                self.record_type_hover(Span::of_node(node), &context.self_type);
                context.self_type.clone()
            }
            Node::ArrayNode { .. } => self.infer_array(node, hint, context, locals),
            Node::HashNode { .. } | Node::KeywordHashNode { .. } => {
                self.infer_hash(node, hint, context, locals)
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
                self.infer_optional_node(parentheses.body(), hint, context, locals)
            }
            Node::StatementsNode { .. } => {
                let Some(statements) = node.as_statements_node() else {
                    return Type::Untyped;
                };
                self.infer_statements(&statements, hint, context, locals)
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
            | Node::MatchWriteNode { .. } => self.infer_variable(node, hint, context, locals),
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
                self.infer_with_hint(&shareable.write(), hint, context, locals)
            }
            Node::CallNode { .. } => {
                let Some(call) = node.as_call_node() else {
                    return Type::Untyped;
                };
                self.infer_call(&call, hint, context, locals)
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
                self.infer_conditional(node, hint, context, locals)
            }
            Node::CaseNode { .. } => self.infer_case(node, hint, context, locals),
            Node::CaseMatchNode { .. } => self.infer_case_match(node, hint, context, locals),
            Node::WhileNode { .. } | Node::UntilNode { .. } => {
                self.infer_loop(node, context, locals)
            }
            Node::ForNode { .. } => self.infer_for(node, context, locals),
            Node::AndNode { .. } => self.infer_and(node, hint, context, locals),
            Node::OrNode { .. } => self.infer_or(node, hint, context, locals),
            Node::BeginNode { .. } => self.infer_begin(node, hint, context, locals),
            Node::RescueModifierNode { .. } => {
                let Some(rescue_modifier) = node.as_rescue_modifier_node() else {
                    return Type::Untyped;
                };
                let expression_type = self.infer_with_hint(&rescue_modifier.expression(), hint, context, locals);
                let rescue_type = self.infer_with_hint(&rescue_modifier.rescue_expression(), hint, context, locals);
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
            Node::LambdaNode { .. } => self.infer_lambda(node, hint, context, locals),
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
