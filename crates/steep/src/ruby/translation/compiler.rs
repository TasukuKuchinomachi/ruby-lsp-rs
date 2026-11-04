use std::rc::Rc;

use ruby_prism::{
    BlockNode, CallNode, ConstantId, Location as PrismLocation, Node as PrismNode, NodeList,
    ParametersNode, StatementsNode,
};

use crate::ruby::node::{Child, Node, SourceRange};
use crate::ruby::translation::builder::{BlockArguments, Builder, Token};
use crate::ruby::translation::numeric::NumericValue;

#[derive(Clone, Copy, Default)]
struct Forwarding {
    rest: bool,
    keyword_rest: bool,
}

#[derive(Clone, Copy)]
enum ComposedString {
    String,
    ExecutableString,
}

enum HeredocPart<'pr> {
    Prism(PrismNode<'pr>),
    Content {
        unescaped: Vec<u8>,
        content: SourceRange,
    },
}

pub(super) struct Compiler<'pr, 'buffer> {
    pub builder: Builder<'buffer>,
    source: &'pr str,
    forwarding: Forwarding,
    in_destructure: bool,
    in_pattern: bool,
}

fn starts_with_any(bytes: Option<&[u8]>, prefixes: &[&str]) -> bool {
    bytes.is_some_and(|bytes| {
        prefixes
            .iter()
            .any(|prefix| bytes.starts_with(prefix.as_bytes()))
    })
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(&bytes[start..=index]);
            start = index + 1;
        }
    }
    if start < bytes.len() {
        lines.push(&bytes[start..]);
    }
    lines
}

fn is_ruby_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn has_backslash_before_final_newline(line: &[u8]) -> bool {
    line.len() >= 2 && line[line.len() - 1] == b'\n' && line[line.len() - 2] == b'\\'
}

fn has_odd_backslashes_before_final_newline(line: &[u8]) -> bool {
    let Some(mut end) = line.len().checked_sub(1) else {
        return false;
    };
    if line[end] != b'\n' {
        return false;
    }
    if end > 0 && line[end - 1] == b'\r' {
        end -= 1;
    }
    let backslashes = line[..end]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count();
    backslashes % 2 == 1
}

fn escaped_newline_count(line: &[u8]) -> usize {
    let mut count = 0;
    let mut backslashes = 0;
    for byte in line {
        match byte {
            b'\\' => backslashes += 1,
            b'n' => {
                if backslashes % 2 == 1 {
                    count += 1;
                }
                backslashes = 0;
            }
            _ => backslashes = 0,
        }
    }
    count
}

fn trailing_space_length(text: &[u8]) -> usize {
    for start in 0..text.len() {
        if !is_ruby_space(text[start]) {
            continue;
        }
        let run_end = start
            + text[start..]
                .iter()
                .take_while(|byte| is_ruby_space(**byte))
                .count();
        if let Some(end) = (start + 1..=run_end)
            .rev()
            .find(|end| *end == text.len() || text[*end] == b'\n')
        {
            return end - start;
        }
    }
    0
}

fn chomp(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .or_else(|| text.strip_suffix('\r'))
        .unwrap_or(text)
}

fn is_procarg0(parameters: Option<&ParametersNode>) -> bool {
    parameters.is_some_and(|parameters| {
        parameters.requireds().len() == 1
            && parameters.optionals().is_empty()
            && parameters.rest().is_none()
            && parameters.posts().is_empty()
            && parameters.keywords().is_empty()
            && parameters.keyword_rest().is_none()
            && parameters.block().is_none()
    })
}

fn find_forwarding(parameters: Option<&ParametersNode>) -> Forwarding {
    let Some(parameters) = parameters else {
        return Forwarding::default();
    };
    Forwarding {
        rest: parameters
            .rest()
            .and_then(|rest| rest.as_rest_parameter_node())
            .is_some_and(|rest| rest.name().is_none()),
        keyword_rest: parameters
            .keyword_rest()
            .and_then(|rest| rest.as_keyword_rest_parameter_node())
            .is_some_and(|rest| rest.name().is_none()),
    }
}

fn multi_target_elements<'pr>(
    lefts: NodeList<'pr>,
    rest: Option<PrismNode<'pr>>,
    rights: NodeList<'pr>,
) -> Vec<PrismNode<'pr>> {
    let mut elements: Vec<PrismNode<'pr>> = lefts.iter().collect();
    if let Some(rest) = rest
        && rest.as_implicit_rest_node().is_none()
    {
        elements.push(rest);
    }
    elements.extend(rights.iter());
    elements
}

impl<'pr, 'buffer> Compiler<'pr, 'buffer> {
    pub fn new(builder: Builder<'buffer>, source: &'pr str) -> Compiler<'pr, 'buffer> {
        Compiler {
            builder,
            source,
            forwarding: Forwarding::default(),
            in_destructure: false,
            in_pattern: false,
        }
    }

    fn with_forwarding<T>(
        &mut self,
        forwarding: Forwarding,
        visit: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let saved = self.forwarding;
        self.forwarding = forwarding;
        let result = visit(self);
        self.forwarding = saved;
        result
    }

    fn with_destructure<T>(&mut self, visit: impl FnOnce(&mut Self) -> T) -> T {
        let saved = self.in_destructure;
        self.in_destructure = true;
        let result = visit(self);
        self.in_destructure = saved;
        result
    }

    fn with_pattern<T>(&mut self, in_pattern: bool, visit: impl FnOnce(&mut Self) -> T) -> T {
        let saved = self.in_pattern;
        self.in_pattern = in_pattern;
        let result = visit(self);
        self.in_pattern = saved;
        result
    }

    fn floor(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.source.len());
        while !self.source.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn offsets(&self, start: usize, end: usize) -> SourceRange {
        SourceRange::new(self.floor(start), self.floor(end))
    }

    fn range(&self, location: &PrismLocation<'pr>) -> SourceRange {
        self.offsets(location.start_offset(), location.end_offset())
    }

    fn bytes(&self, start: usize, end: usize) -> &'pr [u8] {
        let length = self.source.len();
        &self.source.as_bytes()[start.min(length)..end.min(length)]
    }

    fn location_bytes(&self, location: &PrismLocation<'pr>) -> &'pr [u8] {
        self.bytes(location.start_offset(), location.end_offset())
    }

    fn constant(&self, id: ConstantId<'pr>) -> String {
        lossy(id.as_slice())
    }

    fn token(&self, location: Option<PrismLocation<'pr>>) -> Option<Token> {
        location.map(|location| self.token_at(&location))
    }

    fn token_at(&self, location: &PrismLocation<'pr>) -> Token {
        Token::new(lossy(self.location_bytes(location)), self.range(location))
    }

    fn named_token(&self, name: String, location: &PrismLocation<'pr>) -> Token {
        Token::new(name, self.range(location))
    }

    fn call_operator(&self, location: Option<PrismLocation<'pr>>) -> Option<Token> {
        location.map(|location| {
            let value = match self.location_bytes(&location) {
                b"." => "dot",
                b"&." => "anddot",
                _ => "::",
            };
            Token::new(value, self.range(&location))
        })
    }

    fn binary_operator(&self, location: &PrismLocation<'pr>) -> Token {
        let text = lossy(self.location_bytes(location));
        let operator = text.strip_suffix('=').unwrap_or(&text).to_string();
        Token::new(operator, self.range(location))
    }

    fn semicolon(&self, start: usize, end: Option<usize>) -> Option<Token> {
        let end = end.unwrap_or(self.source.len());
        let bytes = self.bytes(start, end);
        let spaces = bytes
            .iter()
            .take_while(|byte| is_ruby_space(**byte))
            .count();
        if bytes.get(spaces) == Some(&b';') {
            let final_offset = start + spaces + 1;
            Some(Token::new(
                ";",
                self.offsets(final_offset - 1, final_offset),
            ))
        } else {
            None
        }
    }

    fn line_start(&self, offset: usize) -> usize {
        self.source.as_bytes()[..offset]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)
    }

    pub fn visit(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        match node {
            PrismNode::AliasGlobalVariableNode { .. } => {
                let node = node.as_alias_global_variable_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let new_name = self.visit(&node.new_name());
                let old_name = self.visit(&node.old_name());
                Some(self.builder.alias(&keyword, new_name, old_name))
            }
            PrismNode::AliasMethodNode { .. } => {
                let node = node.as_alias_method_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let new_name = self.visit(&node.new_name());
                let old_name = self.visit(&node.old_name());
                Some(self.builder.alias(&keyword, new_name, old_name))
            }
            PrismNode::AlternationPatternNode { .. } => {
                let node = node.as_alternation_pattern_node()?;
                let left = self.visit(&node.left())?;
                let operator = self.token_at(&node.operator_loc());
                let right = self.visit(&node.right())?;
                Some(
                    self.builder
                        .binary_pattern("match_alt", left, &operator, right),
                )
            }
            PrismNode::AndNode { .. } => {
                let node = node.as_and_node()?;
                let left = self.visit(&node.left())?;
                let operator = self.token_at(&node.operator_loc());
                let right = self.visit(&node.right())?;
                Some(self.builder.logical_op("and", left, &operator, right))
            }
            PrismNode::OrNode { .. } => {
                let node = node.as_or_node()?;
                let left = self.visit(&node.left())?;
                let operator = self.token_at(&node.operator_loc());
                let right = self.visit(&node.right())?;
                Some(self.builder.logical_op("or", left, &operator, right))
            }
            PrismNode::ArrayNode { .. } => self.visit_array_node(node),
            PrismNode::ArrayPatternNode { .. } => self.visit_array_pattern_node(node),
            PrismNode::AssocNode { .. } => self.visit_assoc_node(node),
            PrismNode::AssocSplatNode { .. } => {
                let node = node.as_assoc_splat_node()?;
                let operator = self.token_at(&node.operator_loc());
                if self.in_pattern {
                    let name = self.token(node.value().map(|value| value.location()));
                    Some(self.builder.match_rest(&operator, name.as_ref()))
                } else if node.value().is_none() && self.forwarding.keyword_rest {
                    Some(self.builder.forwarded_kwrestarg(&operator))
                } else {
                    let value = self.visit_optional(node.value());
                    Some(self.builder.kwsplat(&operator, value))
                }
            }
            PrismNode::BackReferenceReadNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.back_ref(&token))
            }
            PrismNode::BeginNode { .. } => self.visit_begin_node(node),
            PrismNode::BlockArgumentNode { .. } => {
                let node = node.as_block_argument_node()?;
                let operator = self.token_at(&node.operator_loc());
                let expression = self.visit_optional(node.expression());
                Some(self.builder.block_pass(&operator, expression))
            }
            PrismNode::BlockLocalVariableNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.shadowarg(&token))
            }
            PrismNode::BlockParameterNode { .. } => {
                let node = node.as_block_parameter_node()?;
                let operator = self.token_at(&node.operator_loc());
                let name = self.token(node.name_loc());
                Some(self.builder.blockarg(&operator, name.as_ref()))
            }
            PrismNode::BreakNode { .. } => {
                let node = node.as_break_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let arguments = self.visit_arguments(node.arguments());
                Some(
                    self.builder
                        .keyword_cmd("break", &keyword, None, arguments, None),
                )
            }
            PrismNode::NextNode { .. } => {
                let node = node.as_next_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let arguments = self.visit_arguments(node.arguments());
                Some(
                    self.builder
                        .keyword_cmd("next", &keyword, None, arguments, None),
                )
            }
            PrismNode::ReturnNode { .. } => {
                let node = node.as_return_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let arguments = self.visit_arguments(node.arguments());
                Some(
                    self.builder
                        .keyword_cmd("return", &keyword, None, arguments, None),
                )
            }
            PrismNode::RedoNode { .. } => {
                let keyword = self.token_at(&node.location());
                Some(
                    self.builder
                        .keyword_cmd("redo", &keyword, None, Vec::new(), None),
                )
            }
            PrismNode::RetryNode { .. } => {
                let keyword = self.token_at(&node.location());
                Some(
                    self.builder
                        .keyword_cmd("retry", &keyword, None, Vec::new(), None),
                )
            }
            PrismNode::YieldNode { .. } => {
                let node = node.as_yield_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let left_parenthesis = self.token(node.lparen_loc());
                let arguments = self.visit_arguments(node.arguments());
                let right_parenthesis = self.token(node.rparen_loc());
                Some(self.builder.keyword_cmd(
                    "yield",
                    &keyword,
                    left_parenthesis.as_ref(),
                    arguments,
                    right_parenthesis.as_ref(),
                ))
            }
            PrismNode::CallNode { .. } => self.visit_call_node(&node.as_call_node()?),
            PrismNode::CallOperatorWriteNode { .. } => {
                let node = node.as_call_operator_write_node()?;
                let receiver = self.visit_optional(node.receiver());
                let dot = self.call_operator(node.call_operator_loc());
                let selector = node
                    .message_loc()
                    .map(|location| self.named_token(self.constant(node.read_name()), &location));
                let call = self.builder.call_method(
                    receiver,
                    dot.as_ref(),
                    selector.as_ref(),
                    None,
                    Vec::new(),
                    None,
                );
                let operator = self.binary_operator(&node.binary_operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(call, &operator, value))
            }
            PrismNode::CallAndWriteNode { .. } => {
                let node = node.as_call_and_write_node()?;
                let receiver = self.visit_optional(node.receiver());
                let dot = self.call_operator(node.call_operator_loc());
                let selector = node
                    .message_loc()
                    .map(|location| self.named_token(self.constant(node.read_name()), &location));
                let call = self.builder.call_method(
                    receiver,
                    dot.as_ref(),
                    selector.as_ref(),
                    None,
                    Vec::new(),
                    None,
                );
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(call, &operator, value))
            }
            PrismNode::CallOrWriteNode { .. } => {
                let node = node.as_call_or_write_node()?;
                let receiver = self.visit_optional(node.receiver());
                let dot = self.call_operator(node.call_operator_loc());
                let selector = node
                    .message_loc()
                    .map(|location| self.named_token(self.constant(node.read_name()), &location));
                let call = self.builder.call_method(
                    receiver,
                    dot.as_ref(),
                    selector.as_ref(),
                    None,
                    Vec::new(),
                    None,
                );
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(call, &operator, value))
            }
            PrismNode::CallTargetNode { .. } => {
                let node = node.as_call_target_node()?;
                let receiver = self.visit(&node.receiver());
                let dot = self.call_operator(Some(node.call_operator_loc()));
                let selector = self.token_at(&node.message_loc());
                Some(self.builder.attr_asgn(receiver, dot.as_ref(), &selector))
            }
            PrismNode::CapturePatternNode { .. } => {
                let node = node.as_capture_pattern_node()?;
                let value = self.visit(&node.value())?;
                let operator = self.token_at(&node.operator_loc());
                let target = self.visit(&node.target().as_node())?;
                Some(
                    self.builder
                        .binary_pattern("match_as", value, &operator, target),
                )
            }
            PrismNode::CaseNode { .. } => {
                let node = node.as_case_node()?;
                let case = self.token_at(&node.case_keyword_loc());
                let predicate = self.visit_optional(node.predicate());
                let conditions = self.visit_all(node.conditions().iter());
                let else_token =
                    self.token(node.else_clause().map(|clause| clause.else_keyword_loc()));
                let else_body = node
                    .else_clause()
                    .and_then(|clause| self.visit_statements(clause.statements()));
                let end = self.token_at(&node.end_keyword_loc());
                Some(self.builder.case(
                    &case,
                    predicate,
                    conditions,
                    else_token.as_ref(),
                    else_body,
                    &end,
                ))
            }
            PrismNode::CaseMatchNode { .. } => {
                let node = node.as_case_match_node()?;
                let case = self.token_at(&node.case_keyword_loc());
                let predicate = self.visit_optional(node.predicate());
                let conditions = self.visit_all(node.conditions().iter());
                let else_token =
                    self.token(node.else_clause().map(|clause| clause.else_keyword_loc()));
                let else_body = node
                    .else_clause()
                    .and_then(|clause| self.visit_statements(clause.statements()));
                let end = self.token_at(&node.end_keyword_loc());
                Some(self.builder.case_match(
                    &case,
                    predicate,
                    conditions,
                    else_token.as_ref(),
                    else_body,
                    &end,
                ))
            }
            PrismNode::ClassNode { .. } => {
                let node = node.as_class_node()?;
                let class = self.token_at(&node.class_keyword_loc());
                let name = self.visit(&node.constant_path());
                let less_than = self.token(node.inheritance_operator_loc());
                let superclass = self.visit_optional(node.superclass());
                let body = self.with_forwarding(Forwarding::default(), |compiler| {
                    compiler.visit_optional(node.body())
                });
                let end = self.token_at(&node.end_keyword_loc());
                Some(self.builder.def_class(
                    &class,
                    name,
                    less_than.as_ref(),
                    superclass,
                    body,
                    &end,
                ))
            }
            PrismNode::ModuleNode { .. } => {
                let node = node.as_module_node()?;
                let module = self.token_at(&node.module_keyword_loc());
                let name = self.visit(&node.constant_path());
                let body = self.with_forwarding(Forwarding::default(), |compiler| {
                    compiler.visit_optional(node.body())
                });
                let end = self.token_at(&node.end_keyword_loc());
                Some(self.builder.def_module(&module, name, body, &end))
            }
            PrismNode::SingletonClassNode { .. } => {
                let node = node.as_singleton_class_node()?;
                let class = self.token_at(&node.class_keyword_loc());
                let operator = self.token_at(&node.operator_loc());
                let expression = self.visit(&node.expression());
                let body = self.with_forwarding(Forwarding::default(), |compiler| {
                    compiler.visit_optional(node.body())
                });
                let end = self.token_at(&node.end_keyword_loc());
                Some(
                    self.builder
                        .def_sclass(&class, &operator, expression, body, &end),
                )
            }
            PrismNode::ClassVariableReadNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.cvar(&token))
            }
            PrismNode::ClassVariableWriteNode { .. } => {
                let node = node.as_class_variable_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.cvar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.token_at(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.assign(target, Some(&operator), value))
            }
            PrismNode::ClassVariableOperatorWriteNode { .. } => {
                let node = node.as_class_variable_operator_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.cvar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.binary_operator(&node.binary_operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::ClassVariableAndWriteNode { .. } => {
                let node = node.as_class_variable_and_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.cvar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::ClassVariableOrWriteNode { .. } => {
                let node = node.as_class_variable_or_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.cvar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::ClassVariableTargetNode { .. } => {
                let token = self.token_at(&node.location());
                let variable = self.builder.cvar(&token);
                Some(self.builder.assignable(variable))
            }
            PrismNode::ConstantReadNode { .. } => {
                let node = node.as_constant_read_node()?;
                let name = self.named_token(self.constant(node.name()), &node.location());
                Some(self.builder.const_(&name))
            }
            PrismNode::ConstantWriteNode { .. } => {
                let node = node.as_constant_write_node()?;
                let name = self.named_token(self.constant(node.name()), &node.name_loc());
                let constant = self.builder.const_(&name);
                let target = self.builder.assignable(constant);
                let operator = self.token_at(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.assign(target, Some(&operator), value))
            }
            PrismNode::ConstantOperatorWriteNode { .. } => {
                let node = node.as_constant_operator_write_node()?;
                let name = self.named_token(self.constant(node.name()), &node.name_loc());
                let constant = self.builder.const_(&name);
                let target = self.builder.assignable(constant);
                let operator = self.binary_operator(&node.binary_operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::ConstantAndWriteNode { .. } => {
                let node = node.as_constant_and_write_node()?;
                let name = self.named_token(self.constant(node.name()), &node.name_loc());
                let constant = self.builder.const_(&name);
                let target = self.builder.assignable(constant);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::ConstantOrWriteNode { .. } => {
                let node = node.as_constant_or_write_node()?;
                let name = self.named_token(self.constant(node.name()), &node.name_loc());
                let constant = self.builder.const_(&name);
                let target = self.builder.assignable(constant);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::ConstantTargetNode { .. } => {
                let node = node.as_constant_target_node()?;
                let name = self.named_token(self.constant(node.name()), &node.location());
                let constant = self.builder.const_(&name);
                Some(self.builder.assignable(constant))
            }
            PrismNode::ConstantPathNode { .. } => {
                let node = node.as_constant_path_node()?;
                Some(self.visit_constant_path(
                    node.parent(),
                    node.delimiter_loc(),
                    node.name(),
                    node.name_loc(),
                ))
            }
            PrismNode::ConstantPathTargetNode { .. } => {
                let node = node.as_constant_path_target_node()?;
                let constant = self.visit_constant_path(
                    node.parent(),
                    node.delimiter_loc(),
                    node.name(),
                    node.name_loc(),
                );
                Some(self.builder.assignable(constant))
            }
            PrismNode::ConstantPathWriteNode { .. } => {
                let node = node.as_constant_path_write_node()?;
                let constant = self.visit(&node.target().as_node())?;
                let target = self.builder.assignable(constant);
                let operator = self.token_at(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.assign(target, Some(&operator), value))
            }
            PrismNode::ConstantPathOperatorWriteNode { .. } => {
                let node = node.as_constant_path_operator_write_node()?;
                let constant = self.visit(&node.target().as_node())?;
                let target = self.builder.assignable(constant);
                let operator = self.binary_operator(&node.binary_operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::ConstantPathAndWriteNode { .. } => {
                let node = node.as_constant_path_and_write_node()?;
                let constant = self.visit(&node.target().as_node())?;
                let target = self.builder.assignable(constant);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::ConstantPathOrWriteNode { .. } => {
                let node = node.as_constant_path_or_write_node()?;
                let constant = self.visit(&node.target().as_node())?;
                let target = self.builder.assignable(constant);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::DefNode { .. } => self.visit_def_node(node),
            PrismNode::DefinedNode { .. } => {
                let node = node.as_defined_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let multiline_parenthesis = node.lparen_loc().is_some_and(|left_parenthesis| {
                    self.bytes(
                        node.keyword_loc().start_offset(),
                        left_parenthesis.end_offset(),
                    )
                    .contains(&b'\n')
                });
                if multiline_parenthesis {
                    let left_parenthesis = self.token(node.lparen_loc())?;
                    let value = self.visit(&node.value());
                    let right_parenthesis = self.token(node.rparen_loc())?;
                    let body = self
                        .builder
                        .begin(&left_parenthesis, value, &right_parenthesis);
                    Some(
                        self.builder
                            .keyword_cmd("defined?", &keyword, None, vec![body], None),
                    )
                } else {
                    let left_parenthesis = self.token(node.lparen_loc());
                    let value = self.visit(&node.value());
                    let right_parenthesis = self.token(node.rparen_loc());
                    Some(self.builder.keyword_cmd(
                        "defined?",
                        &keyword,
                        left_parenthesis.as_ref(),
                        value.into_iter().collect(),
                        right_parenthesis.as_ref(),
                    ))
                }
            }
            PrismNode::ElseNode { .. } => {
                let node = node.as_else_node()?;
                self.visit_statements(node.statements())
            }
            PrismNode::EmbeddedStatementsNode { .. } => {
                let node = node.as_embedded_statements_node()?;
                let opening = self.token_at(&node.opening_loc());
                let statements = self.visit_statements(node.statements());
                let closing = self.token_at(&node.closing_loc());
                Some(self.builder.begin(&opening, statements, &closing))
            }
            PrismNode::EmbeddedVariableNode { .. } => {
                let node = node.as_embedded_variable_node()?;
                self.visit(&node.variable())
            }
            PrismNode::FalseNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.false_(&token))
            }
            PrismNode::NilNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.nil(&token))
            }
            PrismNode::TrueNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.true_(&token))
            }
            PrismNode::SelfNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.self_(&token))
            }
            PrismNode::FindPatternNode { .. } => {
                let node = node.as_find_pattern_node()?;
                let mut elements = vec![node.left().as_node()];
                elements.extend(node.requireds().iter());
                elements.push(node.right());
                let visited = self.visit_all(elements.into_iter());
                let opening = self.token(node.opening_loc());
                let closing = self.token(node.closing_loc());
                match node.constant() {
                    Some(constant) => {
                        let constant = self.visit(&constant)?;
                        let pattern = self.builder.find_pattern(None, visited, None);
                        Some(self.builder.const_pattern(
                            constant,
                            opening.as_ref(),
                            pattern,
                            closing.as_ref(),
                        ))
                    }
                    None => Some(self.builder.find_pattern(
                        opening.as_ref(),
                        visited,
                        closing.as_ref(),
                    )),
                }
            }
            PrismNode::IntegerNode { .. }
            | PrismNode::FloatNode { .. }
            | PrismNode::RationalNode { .. }
            | PrismNode::ImaginaryNode { .. } => {
                let value = NumericValue::from_node(node)?;
                let range = self.range(&node.location());
                Some(self.visit_numeric(value, range))
            }
            PrismNode::ForNode { .. } => {
                let node = node.as_for_node()?;
                let for_token = self.token_at(&node.for_keyword_loc());
                let index = self.visit(&node.index());
                let in_token = self.token_at(&node.in_keyword_loc());
                let collection = self.visit(&node.collection());
                let do_token = match node.do_keyword_loc() {
                    Some(location) => Some(self.token_at(&location)),
                    None => {
                        let end = node
                            .statements()
                            .map(|statements| statements.location().start_offset())
                            .unwrap_or_else(|| node.end_keyword_loc().start_offset());
                        self.semicolon(node.collection().location().end_offset(), Some(end))
                    }
                };
                let statements = self.visit_statements(node.statements());
                let end = self.token_at(&node.end_keyword_loc());
                Some(self.builder.for_(
                    &for_token,
                    index,
                    &in_token,
                    collection,
                    do_token.as_ref(),
                    statements,
                    &end,
                ))
            }
            PrismNode::ForwardingArgumentsNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.forwarded_args(&token))
            }
            PrismNode::ForwardingParameterNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.forward_arg(&token))
            }
            PrismNode::ForwardingSuperNode { .. } => {
                let node = node.as_forwarding_super_node()?;
                let start = node.location().start_offset();
                let keyword = Token::new("super", self.offsets(start, start + 5));
                let call = self
                    .builder
                    .keyword_cmd("zsuper", &keyword, None, Vec::new(), None);
                Some(self.visit_block(call, node.block()))
            }
            PrismNode::GlobalVariableReadNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.gvar(&token))
            }
            PrismNode::GlobalVariableWriteNode { .. } => {
                let node = node.as_global_variable_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.gvar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.token_at(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.assign(target, Some(&operator), value))
            }
            PrismNode::GlobalVariableOperatorWriteNode { .. } => {
                let node = node.as_global_variable_operator_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.gvar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.binary_operator(&node.binary_operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::GlobalVariableAndWriteNode { .. } => {
                let node = node.as_global_variable_and_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.gvar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::GlobalVariableOrWriteNode { .. } => {
                let node = node.as_global_variable_or_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.gvar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::GlobalVariableTargetNode { .. } => {
                let token = self.token_at(&node.location());
                let variable = self.builder.gvar(&token);
                Some(self.builder.assignable(variable))
            }
            PrismNode::HashNode { .. } => {
                let node = node.as_hash_node()?;
                let opening = self.token_at(&node.opening_loc());
                let elements = self.visit_all(node.elements().iter());
                let closing = self.token_at(&node.closing_loc());
                Some(
                    self.builder
                        .associate(Some(&opening), elements, Some(&closing)),
                )
            }
            PrismNode::HashPatternNode { .. } => {
                let node = node.as_hash_pattern_node()?;
                let mut elements: Vec<PrismNode<'pr>> = node.elements().iter().collect();
                elements.extend(node.rest());
                let visited = self.visit_all(elements.into_iter());
                let opening = self.token(node.opening_loc());
                let closing = self.token(node.closing_loc());
                match node.constant() {
                    Some(constant) => {
                        let constant = self.visit(&constant)?;
                        let pattern = self.builder.hash_pattern(None, visited, None);
                        Some(self.builder.const_pattern(
                            constant,
                            opening.as_ref(),
                            pattern,
                            closing.as_ref(),
                        ))
                    }
                    None => Some(self.builder.hash_pattern(
                        opening.as_ref(),
                        visited,
                        closing.as_ref(),
                    )),
                }
            }
            PrismNode::IfNode { .. } => self.visit_if_node(node),
            PrismNode::UnlessNode { .. } => self.visit_unless_node(node),
            PrismNode::InNode { .. } => self.visit_in_node(node),
            PrismNode::IndexOperatorWriteNode { .. } => {
                let node = node.as_index_operator_write_node()?;
                let mut arguments: Vec<PrismNode<'pr>> = node
                    .arguments()
                    .map(|arguments| arguments.arguments().iter().collect())
                    .unwrap_or_default();
                arguments.extend(node.block().map(|block| block.as_node()));
                let receiver = self.visit_optional(node.receiver());
                let opening = self.token_at(&node.opening_loc());
                let indexes = self.visit_all(arguments.into_iter());
                let closing = self.token_at(&node.closing_loc());
                let index = self
                    .builder
                    .index(receiver, Some(&opening), indexes, Some(&closing));
                let operator = self.binary_operator(&node.binary_operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(index, &operator, value))
            }
            PrismNode::IndexAndWriteNode { .. } => {
                let node = node.as_index_and_write_node()?;
                let mut arguments: Vec<PrismNode<'pr>> = node
                    .arguments()
                    .map(|arguments| arguments.arguments().iter().collect())
                    .unwrap_or_default();
                arguments.extend(node.block().map(|block| block.as_node()));
                let receiver = self.visit_optional(node.receiver());
                let opening = self.token_at(&node.opening_loc());
                let indexes = self.visit_all(arguments.into_iter());
                let closing = self.token_at(&node.closing_loc());
                let index = self
                    .builder
                    .index(receiver, Some(&opening), indexes, Some(&closing));
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(index, &operator, value))
            }
            PrismNode::IndexOrWriteNode { .. } => {
                let node = node.as_index_or_write_node()?;
                let mut arguments: Vec<PrismNode<'pr>> = node
                    .arguments()
                    .map(|arguments| arguments.arguments().iter().collect())
                    .unwrap_or_default();
                arguments.extend(node.block().map(|block| block.as_node()));
                let receiver = self.visit_optional(node.receiver());
                let opening = self.token_at(&node.opening_loc());
                let indexes = self.visit_all(arguments.into_iter());
                let closing = self.token_at(&node.closing_loc());
                let index = self
                    .builder
                    .index(receiver, Some(&opening), indexes, Some(&closing));
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(index, &operator, value))
            }
            PrismNode::IndexTargetNode { .. } => {
                let node = node.as_index_target_node()?;
                let receiver = self.visit(&node.receiver());
                let opening = self.token_at(&node.opening_loc());
                let indexes = match node.arguments() {
                    Some(arguments) => self.visit_all(arguments.arguments().iter()),
                    None => Vec::new(),
                };
                let closing = self.token_at(&node.closing_loc());
                Some(
                    self.builder
                        .index_asgn(receiver, Some(&opening), indexes, Some(&closing)),
                )
            }
            PrismNode::InstanceVariableReadNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.ivar(&token))
            }
            PrismNode::InstanceVariableWriteNode { .. } => {
                let node = node.as_instance_variable_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.ivar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.token_at(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.assign(target, Some(&operator), value))
            }
            PrismNode::InstanceVariableOperatorWriteNode { .. } => {
                let node = node.as_instance_variable_operator_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.ivar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.binary_operator(&node.binary_operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::InstanceVariableAndWriteNode { .. } => {
                let node = node.as_instance_variable_and_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.ivar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::InstanceVariableOrWriteNode { .. } => {
                let node = node.as_instance_variable_or_write_node()?;
                let name = self.token_at(&node.name_loc());
                let variable = self.builder.ivar(&name);
                let target = self.builder.assignable(variable);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::InstanceVariableTargetNode { .. } => {
                let token = self.token_at(&node.location());
                let variable = self.builder.ivar(&token);
                Some(self.builder.assignable(variable))
            }
            PrismNode::InterpolatedRegularExpressionNode { .. } => {
                let node = node.as_interpolated_regular_expression_node()?;
                Some(self.visit_interpolated_regexp(
                    node.opening_loc(),
                    node.parts(),
                    node.closing_loc(),
                ))
            }
            PrismNode::InterpolatedMatchLastLineNode { .. } => {
                let node = node.as_interpolated_match_last_line_node()?;
                Some(self.visit_interpolated_regexp(
                    node.opening_loc(),
                    node.parts(),
                    node.closing_loc(),
                ))
            }
            PrismNode::RegularExpressionNode { .. } => {
                let node = node.as_regular_expression_node()?;
                Some(self.visit_regexp(
                    node.opening_loc(),
                    node.content_loc(),
                    node.closing_loc(),
                    node.unescaped(),
                ))
            }
            PrismNode::MatchLastLineNode { .. } => {
                let node = node.as_match_last_line_node()?;
                Some(self.visit_regexp(
                    node.opening_loc(),
                    node.content_loc(),
                    node.closing_loc(),
                    node.unescaped(),
                ))
            }
            PrismNode::InterpolatedStringNode { .. } => {
                let node = node.as_interpolated_string_node()?;
                let opening = node.opening_loc();
                let opening_bytes = opening
                    .as_ref()
                    .map(|location| self.location_bytes(location));
                if starts_with_any(opening_bytes, &["<<"]) {
                    let parts = node.parts().iter().map(HeredocPart::Prism).collect();
                    return Some(self.visit_heredoc(
                        opening?,
                        parts,
                        node.closing_loc()?,
                        ComposedString::String,
                    ));
                }
                let opening_token = self.token(opening);
                let parts = self.string_nodes_from_interpolation(node.parts(), opening_bytes);
                let closing = self.token(node.closing_loc());
                Some(
                    self.builder
                        .string_compose(opening_token.as_ref(), parts, closing.as_ref()),
                )
            }
            PrismNode::InterpolatedSymbolNode { .. } => {
                let node = node.as_interpolated_symbol_node()?;
                let opening = node.opening_loc();
                let opening_bytes = opening
                    .as_ref()
                    .map(|location| self.location_bytes(location));
                let opening_token = self.token(opening);
                let parts = self.string_nodes_from_interpolation(node.parts(), opening_bytes);
                let closing = self.token(node.closing_loc());
                Some(
                    self.builder
                        .symbol_compose(opening_token.as_ref(), parts, closing.as_ref()),
                )
            }
            PrismNode::InterpolatedXStringNode { .. } => {
                let node = node.as_interpolated_x_string_node()?;
                let opening = node.opening_loc();
                let opening_bytes = self.location_bytes(&opening);
                if opening_bytes.starts_with(b"<<") {
                    let parts = node.parts().iter().map(HeredocPart::Prism).collect();
                    return Some(self.visit_heredoc(
                        opening,
                        parts,
                        node.closing_loc(),
                        ComposedString::ExecutableString,
                    ));
                }
                let opening_token = self.token_at(&opening);
                let parts = self.string_nodes_from_interpolation(node.parts(), Some(opening_bytes));
                let closing = self.token_at(&node.closing_loc());
                Some(
                    self.builder
                        .xstring_compose(Some(&opening_token), parts, Some(&closing)),
                )
            }
            PrismNode::ItLocalVariableReadNode { .. } => {
                let name = self.named_token("it".to_string(), &node.location());
                let identifier = self.builder.ident(&name);
                Some(identifier.updated(Some("lvar"), None, None))
            }
            PrismNode::KeywordHashNode { .. } => {
                let node = node.as_keyword_hash_node()?;
                let elements = self.visit_all(node.elements().iter());
                Some(self.builder.associate(None, elements, None))
            }
            PrismNode::KeywordRestParameterNode { .. } => {
                let node = node.as_keyword_rest_parameter_node()?;
                let operator = self.token_at(&node.operator_loc());
                let name = match (node.name(), node.name_loc()) {
                    (Some(name), Some(location)) => {
                        Some(self.named_token(self.constant(name), &location))
                    }
                    _ => None,
                };
                Some(self.builder.kwrestarg(&operator, name.as_ref()))
            }
            PrismNode::LambdaNode { .. } => self.visit_lambda_node(node),
            PrismNode::LocalVariableReadNode { .. } => {
                let node = node.as_local_variable_read_node()?;
                let name = self.named_token(self.constant(node.name()), &node.location());
                let identifier = self.builder.ident(&name);
                Some(identifier.updated(Some("lvar"), None, None))
            }
            PrismNode::LocalVariableWriteNode { .. } => {
                let node = node.as_local_variable_write_node()?;
                let name = self.token_at(&node.name_loc());
                let identifier = self.builder.ident(&name);
                let target = self.builder.assignable(identifier);
                let operator = self.token_at(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.assign(target, Some(&operator), value))
            }
            PrismNode::LocalVariableOperatorWriteNode { .. } => {
                let node = node.as_local_variable_operator_write_node()?;
                let name = self.token_at(&node.name_loc());
                let identifier = self.builder.ident(&name);
                let target = self.builder.assignable(identifier);
                let operator = self.binary_operator(&node.binary_operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::LocalVariableAndWriteNode { .. } => {
                let node = node.as_local_variable_and_write_node()?;
                let name = self.token_at(&node.name_loc());
                let identifier = self.builder.ident(&name);
                let target = self.builder.assignable(identifier);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::LocalVariableOrWriteNode { .. } => {
                let node = node.as_local_variable_or_write_node()?;
                let name = self.token_at(&node.name_loc());
                let identifier = self.builder.ident(&name);
                let target = self.builder.assignable(identifier);
                let operator = self.binary_operator(&node.operator_loc());
                let value = self.visit(&node.value());
                Some(self.builder.op_assign(target, &operator, value))
            }
            PrismNode::LocalVariableTargetNode { .. } => {
                let node = node.as_local_variable_target_node()?;
                if self.in_pattern {
                    let name = self.named_token(self.constant(node.name()), &node.location());
                    let variable = self.builder.match_var(&name);
                    Some(self.builder.assignable(variable))
                } else {
                    let name = self.token_at(&node.location());
                    let identifier = self.builder.ident(&name);
                    Some(self.builder.assignable(identifier))
                }
            }
            PrismNode::MatchPredicateNode { .. } => {
                let node = node.as_match_predicate_node()?;
                let value = self.visit(&node.value())?;
                let operator = self.token_at(&node.operator_loc());
                let pattern =
                    self.with_pattern(true, |compiler| compiler.visit(&node.pattern()))?;
                Some(
                    self.builder
                        .match_pattern("match_pattern_p", value, &operator, pattern),
                )
            }
            PrismNode::MatchRequiredNode { .. } => {
                let node = node.as_match_required_node()?;
                let value = self.visit(&node.value())?;
                let operator = self.token_at(&node.operator_loc());
                let pattern =
                    self.with_pattern(true, |compiler| compiler.visit(&node.pattern()))?;
                Some(
                    self.builder
                        .match_pattern("match_pattern", value, &operator, pattern),
                )
            }
            PrismNode::MatchWriteNode { .. } => {
                let node = node.as_match_write_node()?;
                let call = node.call();
                let receiver = self.visit_optional(call.receiver())?;
                let operator = self.token(call.message_loc())?;
                let argument = call
                    .arguments()
                    .and_then(|arguments| arguments.arguments().first())
                    .and_then(|argument| self.visit(&argument));
                Some(self.builder.match_op(receiver, &operator, argument))
            }
            PrismNode::MultiTargetNode { .. } => {
                let node = node.as_multi_target_node()?;
                let left_parenthesis = self.token(node.lparen_loc());
                let elements = multi_target_elements(node.lefts(), node.rest(), node.rights());
                let items = self.visit_all(elements.into_iter());
                let right_parenthesis = self.token(node.rparen_loc());
                Some(self.builder.multi_lhs(
                    left_parenthesis.as_ref(),
                    items,
                    right_parenthesis.as_ref(),
                ))
            }
            PrismNode::MultiWriteNode { .. } => {
                let node = node.as_multi_write_node()?;
                let mut elements = multi_target_elements(node.lefts(), node.rest(), node.rights());
                if elements.len() == 1 && node.rest().is_none() {
                    let nested = elements[0].as_multi_target_node();
                    if let Some(nested) = nested {
                        elements =
                            multi_target_elements(nested.lefts(), nested.rest(), nested.rights());
                    }
                }
                let left_parenthesis = self.token(node.lparen_loc());
                let items = self.visit_all(elements.into_iter());
                let right_parenthesis = self.token(node.rparen_loc());
                let left = self.builder.multi_lhs(
                    left_parenthesis.as_ref(),
                    items,
                    right_parenthesis.as_ref(),
                );
                let operator = self.token_at(&node.operator_loc());
                let value = self.visit(&node.value())?;
                Some(self.builder.multi_assign(left, &operator, value))
            }
            PrismNode::NoKeywordsParameterNode { .. } => {
                let node = node.as_no_keywords_parameter_node()?;
                let operator = self.token_at(&node.operator_loc());
                let keyword = self.token_at(&node.keyword_loc());
                if self.in_pattern {
                    Some(self.builder.match_nil_pattern(&operator, &keyword))
                } else {
                    Some(self.builder.kwnilarg(&operator, &keyword))
                }
            }
            PrismNode::NumberedReferenceReadNode { .. } => {
                let node = node.as_numbered_reference_read_node()?;
                let range = self.range(&node.location());
                Some(self.builder.nth_ref(node.number(), range))
            }
            PrismNode::OptionalKeywordParameterNode { .. } => {
                let node = node.as_optional_keyword_parameter_node()?;
                let name = self.named_token(self.constant(node.name()), &node.name_loc());
                let value = self.visit(&node.value())?;
                Some(self.builder.kwoptarg(&name, value))
            }
            PrismNode::OptionalParameterNode { .. } => {
                let node = node.as_optional_parameter_node()?;
                let name = self.token_at(&node.name_loc());
                let operator = self.token_at(&node.operator_loc());
                let value = self.visit(&node.value())?;
                Some(self.builder.optarg(&name, &operator, value))
            }
            PrismNode::ParenthesesNode { .. } => {
                let node = node.as_parentheses_node()?;
                let opening = self.token_at(&node.opening_loc());
                let body = self.visit_optional(node.body());
                let closing = self.token_at(&node.closing_loc());
                Some(self.builder.begin(&opening, body, &closing))
            }
            PrismNode::PinnedExpressionNode { .. } => {
                let node = node.as_pinned_expression_node()?;
                let parts = self.with_pattern(false, |compiler| compiler.visit(&node.expression()));
                let left_parenthesis = self.token_at(&node.lparen_loc());
                let right_parenthesis = self.token_at(&node.rparen_loc());
                let expression = self
                    .builder
                    .begin(&left_parenthesis, parts, &right_parenthesis);
                let operator = self.token_at(&node.operator_loc());
                Some(self.builder.pin(&operator, Some(expression)))
            }
            PrismNode::PinnedVariableNode { .. } => {
                let node = node.as_pinned_variable_node()?;
                let operator = self.token_at(&node.operator_loc());
                let variable = self.visit(&node.variable());
                Some(self.builder.pin(&operator, variable))
            }
            PrismNode::PostExecutionNode { .. } => {
                let node = node.as_post_execution_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let opening = self.token_at(&node.opening_loc());
                let statements = self.visit_statements(node.statements());
                let closing = self.token_at(&node.closing_loc());
                Some(
                    self.builder
                        .preexe("postexe", &keyword, &opening, statements, &closing),
                )
            }
            PrismNode::PreExecutionNode { .. } => {
                let node = node.as_pre_execution_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let opening = self.token_at(&node.opening_loc());
                let statements = self.visit_statements(node.statements());
                let closing = self.token_at(&node.closing_loc());
                Some(
                    self.builder
                        .preexe("preexe", &keyword, &opening, statements, &closing),
                )
            }
            PrismNode::ProgramNode { .. } => {
                let node = node.as_program_node()?;
                self.visit_statements_node(&node.statements())
            }
            PrismNode::RangeNode { .. } => {
                let node = node.as_range_node()?;
                let left = self.visit_optional(node.left());
                let operator = self.token_at(&node.operator_loc());
                let right = self.visit_optional(node.right());
                if node.is_exclude_end() {
                    Some(self.builder.range_exclusive(left, &operator, right))
                } else {
                    Some(self.builder.range_inclusive(left, &operator, right))
                }
            }
            PrismNode::FlipFlopNode { .. } => {
                let node = node.as_flip_flop_node()?;
                let left = self.visit_optional(node.left());
                let operator = self.token_at(&node.operator_loc());
                let right = self.visit_optional(node.right());
                if node.is_exclude_end() {
                    Some(self.builder.range_exclusive(left, &operator, right))
                } else {
                    Some(self.builder.range_inclusive(left, &operator, right))
                }
            }
            PrismNode::RequiredKeywordParameterNode { .. } => {
                let node = node.as_required_keyword_parameter_node()?;
                let name = self.named_token(self.constant(node.name()), &node.name_loc());
                Some(self.builder.kwarg(&name))
            }
            PrismNode::RequiredParameterNode { .. } => {
                let token = self.token_at(&node.location());
                Some(self.builder.arg(&token))
            }
            PrismNode::RescueModifierNode { .. } => {
                let node = node.as_rescue_modifier_node()?;
                let expression = self.visit(&node.expression());
                let keyword = self.token_at(&node.keyword_loc());
                let rescue_expression = self.visit(&node.rescue_expression());
                let rescue_body =
                    self.builder
                        .rescue_body(&keyword, None, None, None, None, rescue_expression);
                self.builder
                    .begin_body(expression, vec![rescue_body], None, None, None, None)
            }
            PrismNode::RestParameterNode { .. } => {
                let node = node.as_rest_parameter_node()?;
                let operator = self.token_at(&node.operator_loc());
                let name = self.token(node.name_loc());
                Some(self.builder.restarg(&operator, name.as_ref()))
            }
            PrismNode::ShareableConstantNode { .. } => {
                let node = node.as_shareable_constant_node()?;
                self.visit(&node.write())
            }
            PrismNode::SourceEncodingNode { .. } => {
                let token = self.token_at(&node.location());
                let encoding = self.builder.source_encoding(&token);
                Some(self.builder.accessible(encoding))
            }
            PrismNode::SourceFileNode { .. } => {
                let token = self.token_at(&node.location());
                let file = self.builder.source_file(&token);
                Some(self.builder.accessible(file))
            }
            PrismNode::SourceLineNode { .. } => {
                let token = self.token_at(&node.location());
                let line = self.builder.source_line(&token);
                Some(self.builder.accessible(line))
            }
            PrismNode::SplatNode { .. } => {
                let node = node.as_splat_node()?;
                let operator = self.token_at(&node.operator_loc());
                if node.expression().is_none() && self.forwarding.rest {
                    Some(self.builder.forwarded_restarg(&operator))
                } else if self.in_destructure {
                    let name =
                        self.token(node.expression().map(|expression| expression.location()));
                    Some(self.builder.restarg(&operator, name.as_ref()))
                } else if self.in_pattern {
                    let name =
                        self.token(node.expression().map(|expression| expression.location()));
                    Some(self.builder.match_rest(&operator, name.as_ref()))
                } else {
                    let expression = self.visit_optional(node.expression());
                    Some(self.builder.splat(&operator, expression))
                }
            }
            PrismNode::StatementsNode { .. } => {
                self.visit_statements_node(&node.as_statements_node()?)
            }
            PrismNode::StringNode { .. } => self.visit_string_node(node),
            PrismNode::SuperNode { .. } => {
                let node = node.as_super_node()?;
                let mut arguments: Vec<PrismNode<'pr>> = node
                    .arguments()
                    .map(|arguments| arguments.arguments().iter().collect())
                    .unwrap_or_default();
                let mut block = node.block();
                if block
                    .as_ref()
                    .is_some_and(|block| block.as_block_argument_node().is_some())
                {
                    arguments.extend(block.take());
                }
                let keyword = self.token_at(&node.keyword_loc());
                let left_parenthesis = self.token(node.lparen_loc());
                let visited = self.visit_all(arguments.into_iter());
                let right_parenthesis = self.token(node.rparen_loc());
                let call = self.builder.keyword_cmd(
                    "super",
                    &keyword,
                    left_parenthesis.as_ref(),
                    visited,
                    right_parenthesis.as_ref(),
                );
                Some(self.visit_block(call, block.and_then(|block| block.as_block_node())))
            }
            PrismNode::SymbolNode { .. } => self.visit_symbol_node(node),
            PrismNode::UndefNode { .. } => {
                let node = node.as_undef_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let names = self.visit_all(node.names().iter());
                Some(self.builder.undef_method(&keyword, names))
            }
            PrismNode::UntilNode { .. } => {
                let node = node.as_until_node()?;
                self.visit_loop(
                    "until",
                    node.location(),
                    node.keyword_loc(),
                    node.predicate(),
                    node.do_keyword_loc(),
                    node.statements(),
                    node.closing_loc(),
                )
            }
            PrismNode::WhileNode { .. } => {
                let node = node.as_while_node()?;
                self.visit_loop(
                    "while",
                    node.location(),
                    node.keyword_loc(),
                    node.predicate(),
                    node.do_keyword_loc(),
                    node.statements(),
                    node.closing_loc(),
                )
            }
            PrismNode::WhenNode { .. } => {
                let node = node.as_when_node()?;
                let keyword = self.token_at(&node.keyword_loc());
                let conditions = self.visit_all(node.conditions().iter());
                let then = match node.then_keyword_loc() {
                    Some(location) => Some(self.token_at(&location)),
                    None => {
                        let start = node
                            .conditions()
                            .last()
                            .map_or(0, |condition| condition.location().end_offset());
                        let end = node
                            .statements()
                            .map(|statements| statements.location().start_offset());
                        self.semicolon(start, end)
                    }
                };
                let statements = self.visit_statements(node.statements());
                Some(
                    self.builder
                        .when(&keyword, conditions, then.as_ref(), statements),
                )
            }
            PrismNode::XStringNode { .. } => {
                let node = node.as_x_string_node()?;
                let opening = node.opening_loc();
                if self.location_bytes(&opening).starts_with(b"<<") {
                    let parts = vec![HeredocPart::Content {
                        unescaped: node.unescaped().to_vec(),
                        content: self.range(&node.content_loc()),
                    }];
                    return Some(self.visit_heredoc(
                        opening,
                        parts,
                        node.closing_loc(),
                        ComposedString::ExecutableString,
                    ));
                }
                let opening_bytes = self.location_bytes(&opening);
                let content = node.content_loc();
                let content_bytes = self.location_bytes(&content);
                let parts = if content_bytes.is_empty() {
                    Vec::new()
                } else if content_bytes.contains(&b'\n') {
                    self.string_nodes_from_line_continuations(
                        node.unescaped(),
                        content_bytes,
                        content.start_offset(),
                        Some(opening_bytes),
                    )
                } else {
                    let range = self.range(&content);
                    vec![self.builder.string_internal(lossy(node.unescaped()), range)]
                };
                let opening_token = self.token_at(&opening);
                let closing = self.token_at(&node.closing_loc());
                Some(
                    self.builder
                        .xstring_compose(Some(&opening_token), parts, Some(&closing)),
                )
            }
            PrismNode::ItParametersNode { .. }
            | PrismNode::NumberedParametersNode { .. }
            | PrismNode::ArgumentsNode { .. }
            | PrismNode::BlockParametersNode { .. }
            | PrismNode::ParametersNode { .. }
            | PrismNode::BlockNode { .. }
            | PrismNode::EnsureNode { .. }
            | PrismNode::ImplicitNode { .. }
            | PrismNode::ImplicitRestNode { .. }
            | PrismNode::RescueNode { .. }
            | PrismNode::MissingNode { .. } => None,
        }
    }

    fn visit_optional(&mut self, node: Option<PrismNode<'pr>>) -> Option<Rc<Node>> {
        node.and_then(|node| self.visit(&node))
    }

    fn visit_all(&mut self, nodes: impl Iterator<Item = PrismNode<'pr>>) -> Vec<Rc<Node>> {
        nodes.filter_map(|node| self.visit(&node)).collect()
    }

    fn visit_statements(&mut self, node: Option<StatementsNode<'pr>>) -> Option<Rc<Node>> {
        node.and_then(|node| self.visit_statements_node(&node))
    }

    fn visit_statements_node(&mut self, node: &StatementsNode<'pr>) -> Option<Rc<Node>> {
        let statements = self.visit_all(node.body().iter());
        self.builder.compstmt(statements)
    }

    fn visit_arguments(&mut self, node: Option<ruby_prism::ArgumentsNode<'pr>>) -> Vec<Rc<Node>> {
        match node {
            Some(node) => self.visit_all(node.arguments().iter()),
            None => Vec::new(),
        }
    }

    fn visit_numeric(&mut self, value: NumericValue, range: SourceRange) -> Rc<Node> {
        let (kind, child) = value.child();
        let numeric = self.builder.numeric(kind, child, range);
        match self.source.as_bytes().get(range.begin_pos) {
            Some(b'+' | b'-') => {
                let operator = self.offsets(range.begin_pos, range.begin_pos + 1);
                self.builder.unary_num(operator, numeric)
            }
            _ => numeric,
        }
    }

    fn visit_constant_path(
        &mut self,
        parent: Option<PrismNode<'pr>>,
        delimiter: PrismLocation<'pr>,
        name: Option<ConstantId<'pr>>,
        name_location: PrismLocation<'pr>,
    ) -> Rc<Node> {
        let name = match name {
            Some(name) => self.named_token(self.constant(name), &name_location),
            None => self.token_at(&name_location),
        };
        let delimiter = self.token_at(&delimiter);
        match parent {
            None => self.builder.const_global(&delimiter, &name),
            Some(parent) => {
                let scope = self.visit(&parent);
                self.builder.const_fetch(scope, &delimiter, &name)
            }
        }
    }

    fn visit_array_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_array_node()?;
        let opening = node.opening_loc();
        let opening_bytes = opening
            .as_ref()
            .map(|location| self.location_bytes(location));
        let elements = if starts_with_any(opening_bytes, &["%w", "%W", "%i", "%I"]) {
            let mut elements = Vec::new();
            for element in node.elements().iter() {
                if let Some(string) = element.as_string_node() {
                    let content = string.content_loc();
                    let content_bytes = self.location_bytes(&content);
                    if content_bytes.contains(&b'\n') {
                        elements.extend(self.string_nodes_from_line_continuations(
                            string.unescaped(),
                            content_bytes,
                            content.start_offset(),
                            opening_bytes,
                        ));
                    } else {
                        let range = self.range(&content);
                        elements.push(
                            self.builder
                                .string_internal(lossy(string.unescaped()), range),
                        );
                    }
                } else if let Some(interpolated) = element.as_interpolated_string_node() {
                    let element_opening = self.token(interpolated.opening_loc());
                    let parts =
                        self.string_nodes_from_interpolation(interpolated.parts(), opening_bytes);
                    let element_closing = self.token(interpolated.closing_loc());
                    elements.push(self.builder.string_compose(
                        element_opening.as_ref(),
                        parts,
                        element_closing.as_ref(),
                    ));
                } else {
                    elements.extend(self.visit(&element));
                }
            }
            elements
        } else {
            self.visit_all(node.elements().iter())
        };
        let opening = self.token(opening);
        let closing = self.token(node.closing_loc());
        Some(
            self.builder
                .array(opening.as_ref(), elements, closing.as_ref()),
        )
    }

    fn visit_array_pattern_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_array_pattern_node()?;
        let mut elements: Vec<PrismNode<'pr>> = node.requireds().iter().collect();
        let rest = node.rest();
        let implicit_rest = rest
            .as_ref()
            .and_then(|rest| rest.as_implicit_rest_node())
            .map(|rest| rest.location());
        if let Some(rest) = rest
            && implicit_rest.is_none()
        {
            elements.push(rest);
        }
        elements.extend(node.posts().iter());
        let mut visited = self.visit_all(elements.into_iter());
        if let Some(implicit_rest) = implicit_rest
            && let Some(last) = visited.pop()
        {
            let comma = self.token_at(&implicit_rest);
            visited.push(self.builder.match_with_trailing_comma(last, &comma));
        }
        let opening = self.token(node.opening_loc());
        let closing = self.token(node.closing_loc());
        match node.constant() {
            Some(constant) => {
                let constant = self.visit(&constant)?;
                let pattern = if visited.is_empty() {
                    self.builder
                        .array_pattern(opening.as_ref(), visited, closing.as_ref())
                } else {
                    self.builder.array_pattern(None, visited, None)
                };
                Some(self.builder.const_pattern(
                    constant,
                    opening.as_ref(),
                    pattern,
                    closing.as_ref(),
                ))
            }
            None => Some(
                self.builder
                    .array_pattern(opening.as_ref(), visited, closing.as_ref()),
            ),
        }
    }

    fn visit_assoc_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_assoc_node()?;
        let key = node.key();
        let value = node.value();
        if let Some(implicit) = value.as_implicit_node() {
            if self.in_pattern {
                if let Some(symbol) = key.as_symbol_node() {
                    match symbol.opening_loc() {
                        None => {
                            let name = Token::new(
                                lossy(symbol.unescaped()),
                                self.range(&symbol.location()),
                            );
                            Some(self.builder.match_hash_var(&name))
                        }
                        Some(opening) => {
                            let opening = self.token_at(&opening);
                            let value_range = symbol
                                .value_loc()
                                .map(|location| self.range(&location))
                                .unwrap_or(SourceRange::new(0, 0));
                            let string = self
                                .builder
                                .string_internal(lossy(symbol.unescaped()), value_range);
                            let closing = self.token(symbol.closing_loc())?;
                            self.builder
                                .match_hash_var_from_str(&opening, vec![string], &closing)
                        }
                    }
                } else {
                    let interpolated = key.as_interpolated_symbol_node()?;
                    let opening = self.token(interpolated.opening_loc())?;
                    let parts = self.visit_all(interpolated.parts().iter());
                    let closing = self.token(interpolated.closing_loc())?;
                    self.builder
                        .match_hash_var_from_str(&opening, parts, &closing)
                }
            } else {
                let symbol = key.as_symbol_node()?;
                let implicit_value = implicit.value();
                let value_range = symbol
                    .value_loc()
                    .map(|location| self.range(&location))
                    .unwrap_or(SourceRange::new(0, 0));
                let implicit_value = if let Some(call) = implicit_value.as_call_node() {
                    let selector = call
                        .message_loc()
                        .map(|location| self.named_token(self.constant(call.name()), &location));
                    self.builder
                        .call_method(None, None, selector.as_ref(), None, Vec::new(), None)
                } else if let Some(constant) = implicit_value.as_constant_read_node() {
                    let name = Token::new(self.constant(constant.name()), value_range);
                    self.builder.const_(&name)
                } else {
                    let name = match implicit_value.as_local_variable_read_node() {
                        Some(variable) => self.constant(variable.name()),
                        None => "it".to_string(),
                    };
                    let identifier = self.builder.ident(&Token::new(name, value_range));
                    identifier.updated(Some("lvar"), None, None)
                };
                let key = Token::new(lossy(symbol.unescaped()), self.range(&symbol.location()));
                Some(self.builder.pair_keyword(&key, implicit_value))
            }
        } else if let Some(operator) = node.operator_loc() {
            let key = self.visit(&key)?;
            let operator = self.token_at(&operator);
            let value = self.visit(&value)?;
            Some(self.builder.pair(key, &operator, value))
        } else if let Some(symbol) = key.as_symbol_node()
            && symbol.opening_loc().is_none()
        {
            let key = Token::new(lossy(symbol.unescaped()), self.range(&symbol.location()));
            let value = self.visit(&value)?;
            Some(self.builder.pair_keyword(&key, value))
        } else {
            let (opening, closing, parts) = if let Some(symbol) = key.as_symbol_node() {
                let value_range = symbol
                    .value_loc()
                    .map(|location| self.range(&location))
                    .unwrap_or(SourceRange::new(0, 0));
                let string = self
                    .builder
                    .string_internal(lossy(symbol.unescaped()), value_range);
                (symbol.opening_loc(), symbol.closing_loc(), vec![string])
            } else {
                let interpolated = key.as_interpolated_symbol_node()?;
                let parts = self.visit_all(interpolated.parts().iter());
                (
                    interpolated.opening_loc(),
                    interpolated.closing_loc(),
                    parts,
                )
            };
            let opening = self.token(opening)?;
            let closing = self.token(closing)?;
            let value = self.visit(&value)?;
            Some(self.builder.pair_quoted(&opening, parts, &closing, value))
        }
    }

    fn visit_begin_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_begin_node()?;
        let mut rescue_bodies = Vec::new();
        let mut rescue_clause = node.rescue_clause();
        while let Some(clause) = rescue_clause {
            let find_start_offset = clause
                .reference()
                .map(|reference| reference.location().end_offset())
                .or_else(|| {
                    clause
                        .exceptions()
                        .last()
                        .map(|exception| exception.location().end_offset())
                })
                .unwrap_or_else(|| clause.keyword_loc().end_offset());
            let find_end_offset = clause
                .statements()
                .map(|statements| statements.location().start_offset())
                .or_else(|| {
                    clause
                        .subsequent()
                        .map(|subsequent| subsequent.location().start_offset())
                })
                .or_else(|| {
                    node.else_clause()
                        .map(|else_clause| else_clause.location().start_offset())
                })
                .or_else(|| {
                    node.ensure_clause()
                        .map(|ensure_clause| ensure_clause.location().start_offset())
                })
                .or_else(|| {
                    node.end_keyword_loc()
                        .map(|location| location.start_offset())
                })
                .unwrap_or(find_start_offset + 1);
            let keyword = self.token_at(&clause.keyword_loc());
            let exceptions = if clause.exceptions().is_empty() {
                None
            } else {
                let exceptions = self.visit_all(clause.exceptions().iter());
                Some(self.builder.array(None, exceptions, None))
            };
            let operator = self.token(clause.operator_loc());
            let reference = self.visit_optional(clause.reference());
            let then = self.semicolon(find_start_offset, Some(find_end_offset));
            let statements = self.visit_statements(clause.statements());
            rescue_bodies.push(self.builder.rescue_body(
                &keyword,
                exceptions,
                operator.as_ref(),
                reference,
                then.as_ref(),
                statements,
            ));
            rescue_clause = clause.subsequent();
        }
        let statements = self.visit_statements(node.statements());
        let else_token = self.token(node.else_clause().map(|clause| clause.else_keyword_loc()));
        let else_body = node
            .else_clause()
            .and_then(|clause| self.visit_statements(clause.statements()));
        let ensure_token = self.token(
            node.ensure_clause()
                .map(|clause| clause.ensure_keyword_loc()),
        );
        let ensure_body = node
            .ensure_clause()
            .and_then(|clause| self.visit_statements(clause.statements()));
        let begin_body = self.builder.begin_body(
            statements,
            rescue_bodies,
            else_token.as_ref(),
            else_body,
            ensure_token.as_ref(),
            ensure_body,
        );
        match node.begin_keyword_loc() {
            Some(begin) => {
                let begin = self.token_at(&begin);
                let end = self.token(node.end_keyword_loc());
                Some(self.builder.begin_keyword(&begin, begin_body, end.as_ref()))
            }
            None => begin_body,
        }
    }

    fn visit_call_node(&mut self, node: &CallNode<'pr>) -> Option<Rc<Node>> {
        let name = self.constant(node.name());
        let mut arguments: Vec<PrismNode<'pr>> = node
            .arguments()
            .map(|arguments| arguments.arguments().iter().collect())
            .unwrap_or_default();
        let mut block = node.block();
        if block
            .as_ref()
            .is_some_and(|block| block.as_block_argument_node().is_some())
        {
            arguments.extend(block.take());
        }
        let block = block.and_then(|block| block.as_block_node());

        if node.call_operator_loc().is_none() {
            match name.as_str() {
                "-@" => {
                    if let Some(receiver) = node.receiver()
                        && let Some(value) = NumericValue::from_node(&receiver)
                        && let Some(message) = node.message_loc()
                    {
                        let range = self.range(&message).join(self.range(&receiver.location()));
                        return Some(self.visit_numeric(value.negate(), range));
                    }
                }
                "!" => {
                    let not = self.token(node.message_loc())?;
                    let opening = self.token(node.opening_loc());
                    let receiver = self.visit_optional(node.receiver());
                    let closing = self.token(node.closing_loc());
                    let call =
                        self.builder
                            .not_op(&not, opening.as_ref(), receiver, closing.as_ref());
                    return Some(self.visit_block(call, block));
                }
                "=~" => {
                    if let Some(receiver) = node.receiver()
                        && receiver.as_regular_expression_node().is_some()
                    {
                        let receiver = self.visit(&receiver)?;
                        let operator = self.token(node.message_loc())?;
                        let argument = node
                            .arguments()
                            .and_then(|arguments| arguments.arguments().first())
                            .and_then(|argument| self.visit(&argument));
                        return Some(self.builder.match_op(receiver, &operator, argument));
                    }
                }
                "[]" => {
                    let receiver = self.visit_optional(node.receiver());
                    let opening = self.token(node.opening_loc());
                    let indexes = self.visit_all(arguments.into_iter());
                    let closing = self.token(node.closing_loc());
                    let call =
                        self.builder
                            .index(receiver, opening.as_ref(), indexes, closing.as_ref());
                    return Some(self.visit_block(call, block));
                }
                "[]=" => {
                    let message = node
                        .message_loc()
                        .map(|location| self.location_bytes(&location));
                    if message != Some(b"[]=".as_slice())
                        && block.is_none()
                        && !node.is_safe_navigation()
                        && let Some(call_arguments) = node.arguments()
                    {
                        let mut index_arguments: Vec<PrismNode<'pr>> =
                            call_arguments.arguments().iter().collect();
                        let value = index_arguments.pop();
                        index_arguments.extend(node.block());
                        let receiver = self.visit_optional(node.receiver());
                        let opening = self.token(node.opening_loc());
                        let indexes = self.visit_all(index_arguments.into_iter());
                        let closing = self.token(node.closing_loc());
                        let target = self.builder.index_asgn(
                            receiver,
                            opening.as_ref(),
                            indexes,
                            closing.as_ref(),
                        );
                        let equal = self.token(node.equal_loc());
                        let value = self.visit_optional(value);
                        return Some(self.builder.assign(target, equal.as_ref(), value));
                    }
                }
                _ => {}
            }
        }

        let message_location = node.message_loc();
        let call_operator = self.call_operator(node.call_operator_loc());
        let is_attribute_assignment = name.ends_with('=')
            && message_location
                .as_ref()
                .is_some_and(|location| !self.location_bytes(location).ends_with(b"="))
            && node.arguments().is_some()
            && block.is_none();
        let call = if is_attribute_assignment {
            let receiver = self.visit_optional(node.receiver());
            let selector = self.token(message_location)?;
            let target = self
                .builder
                .attr_asgn(receiver, call_operator.as_ref(), &selector);
            let equal = self.token(node.equal_loc());
            let value = node
                .arguments()
                .and_then(|arguments| arguments.arguments().last())
                .and_then(|value| self.visit(&value));
            self.builder.assign(target, equal.as_ref(), value)
        } else {
            let receiver = self.visit_optional(node.receiver());
            let selector = message_location
                .as_ref()
                .map(|location| self.named_token(name.clone(), location));
            let opening = self.token(node.opening_loc());
            let arguments = self.visit_all(arguments.into_iter());
            let closing = self.token(node.closing_loc());
            self.builder.call_method(
                receiver,
                call_operator.as_ref(),
                selector.as_ref(),
                opening.as_ref(),
                arguments,
                closing.as_ref(),
            )
        };
        Some(self.visit_block(call, block))
    }

    fn visit_def_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_def_node()?;
        let def = self.token_at(&node.def_keyword_loc());
        let receiver = node
            .receiver()
            .map(|receiver| match receiver.as_parentheses_node() {
                Some(parentheses) => parentheses.body(),
                None => Some(receiver),
            });
        let definee = receiver.map(|receiver| self.visit_optional(receiver));
        let operator = self.token(node.operator_loc());
        let name = self.token_at(&node.name_loc());
        let left_parenthesis = self.token(node.lparen_loc());
        let parameters = match node.parameters() {
            Some(parameters) => self.visit_parameters_node(&parameters),
            None => Vec::new(),
        };
        let right_parenthesis = self.token(node.rparen_loc());
        let arguments = self.builder.args(
            left_parenthesis.as_ref(),
            parameters,
            right_parenthesis.as_ref(),
        );
        let forwarding = find_forwarding(node.parameters().as_ref());
        let body =
            self.with_forwarding(forwarding, |compiler| compiler.visit_optional(node.body()));
        match (node.equal_loc(), definee) {
            (Some(equal), Some(definee)) => {
                let equal = self.token_at(&equal);
                Some(self.builder.def_endless_singleton(
                    &def,
                    definee,
                    operator.as_ref(),
                    &name,
                    arguments,
                    Some(&equal),
                    body,
                ))
            }
            (Some(equal), None) => {
                let equal = self.token_at(&equal);
                Some(
                    self.builder
                        .def_endless_method(&def, &name, arguments, Some(&equal), body),
                )
            }
            (None, Some(definee)) => {
                let end = self.token(node.end_keyword_loc());
                Some(self.builder.def_singleton(
                    &def,
                    definee,
                    operator.as_ref(),
                    &name,
                    arguments,
                    body,
                    end.as_ref(),
                ))
            }
            (None, None) => {
                let end = self.token(node.end_keyword_loc());
                Some(
                    self.builder
                        .def_method(&def, &name, arguments, body, end.as_ref()),
                )
            }
        }
    }

    fn visit_if_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_if_node()?;
        let Some(if_keyword) = node.if_keyword_loc() else {
            let predicate = self.visit(&node.predicate())?;
            let question = self.token(node.then_keyword_loc());
            let statements = self.visit_statements(node.statements());
            let subsequent = node.subsequent();
            let colon = self.token(
                subsequent
                    .as_ref()
                    .and_then(|subsequent| subsequent.as_else_node())
                    .map(|else_node| else_node.else_keyword_loc()),
            );
            let if_false = self.visit_optional(subsequent);
            return Some(self.builder.ternary(
                predicate,
                question.as_ref(),
                statements,
                colon.as_ref(),
                if_false,
            ));
        };
        if if_keyword.start_offset() == node.location().start_offset() {
            let keyword = self.token_at(&if_keyword);
            let predicate = self.visit(&node.predicate())?;
            let then = match node.then_keyword_loc() {
                Some(location) => Some(self.token_at(&location)),
                None => {
                    let end = node
                        .statements()
                        .map(|statements| statements.location().start_offset())
                        .or_else(|| {
                            node.subsequent()
                                .map(|subsequent| subsequent.location().start_offset())
                        })
                        .or_else(|| {
                            node.end_keyword_loc()
                                .map(|location| location.start_offset())
                        });
                    self.semicolon(node.predicate().location().end_offset(), end)
                }
            };
            let statements = self.visit_statements(node.statements());
            let subsequent = node.subsequent();
            let else_token = subsequent.as_ref().and_then(|subsequent| {
                if let Some(if_node) = subsequent.as_if_node() {
                    self.token(if_node.if_keyword_loc())
                } else {
                    subsequent
                        .as_else_node()
                        .map(|else_node| self.token_at(&else_node.else_keyword_loc()))
                }
            });
            let if_false = self.visit_optional(subsequent);
            let is_elsif = self.location_bytes(&if_keyword) == b"elsif";
            let end = if is_elsif {
                None
            } else {
                self.token(node.end_keyword_loc())
            };
            Some(self.builder.condition(
                &keyword,
                predicate,
                then.as_ref(),
                statements,
                else_token.as_ref(),
                if_false,
                end.as_ref(),
            ))
        } else {
            let statements = self.visit_statements(node.statements());
            let subsequent = self.visit_optional(node.subsequent());
            let keyword = self.token_at(&if_keyword);
            let predicate = self.visit(&node.predicate())?;
            Some(
                self.builder
                    .condition_mod(statements, subsequent, &keyword, predicate),
            )
        }
    }

    fn visit_unless_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_unless_node()?;
        let keyword_location = node.keyword_loc();
        if keyword_location.start_offset() == node.location().start_offset() {
            let keyword = self.token_at(&keyword_location);
            let predicate = self.visit(&node.predicate())?;
            let then = match node.then_keyword_loc() {
                Some(location) => Some(self.token_at(&location)),
                None => {
                    let end = node
                        .statements()
                        .map(|statements| statements.location().start_offset())
                        .or_else(|| {
                            node.else_clause()
                                .map(|else_clause| else_clause.location().start_offset())
                        })
                        .or_else(|| {
                            node.end_keyword_loc()
                                .map(|location| location.start_offset())
                        });
                    self.semicolon(node.predicate().location().end_offset(), end)
                }
            };
            let else_body = node
                .else_clause()
                .and_then(|clause| self.visit_statements(clause.statements()));
            let else_token = self.token(node.else_clause().map(|clause| clause.else_keyword_loc()));
            let statements = self.visit_statements(node.statements());
            let end = self.token(node.end_keyword_loc());
            Some(self.builder.condition(
                &keyword,
                predicate,
                then.as_ref(),
                else_body,
                else_token.as_ref(),
                statements,
                end.as_ref(),
            ))
        } else {
            let else_body = node
                .else_clause()
                .and_then(|clause| self.visit_statements(clause.statements()));
            let statements = self.visit_statements(node.statements());
            let keyword = self.token_at(&keyword_location);
            let predicate = self.visit(&node.predicate())?;
            Some(
                self.builder
                    .condition_mod(else_body, statements, &keyword, predicate),
            )
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn visit_loop(
        &mut self,
        type_: &'static str,
        location: PrismLocation<'pr>,
        keyword: PrismLocation<'pr>,
        predicate: PrismNode<'pr>,
        do_keyword: Option<PrismLocation<'pr>>,
        statements: Option<StatementsNode<'pr>>,
        closing: Option<PrismLocation<'pr>>,
    ) -> Option<Rc<Node>> {
        if location.start_offset() == keyword.start_offset() {
            let keyword = self.token_at(&keyword);
            let condition = self.visit(&predicate)?;
            let do_token = match do_keyword {
                Some(location) => Some(self.token_at(&location)),
                None => {
                    let end = statements
                        .as_ref()
                        .map(|statements| statements.location().start_offset())
                        .or_else(|| closing.as_ref().map(|location| location.start_offset()));
                    self.semicolon(predicate.location().end_offset(), end)
                }
            };
            let body = self.visit_statements(statements);
            let end = self.token(closing);
            Some(self.builder.loop_(
                type_,
                &keyword,
                condition,
                do_token.as_ref(),
                body,
                end.as_ref(),
            ))
        } else {
            let body = self.visit_statements(statements);
            let keyword = self.token_at(&keyword);
            let condition = self.visit(&predicate)?;
            Some(self.builder.loop_mod(type_, body, &keyword, condition))
        }
    }

    fn visit_in_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_in_node()?;
        let pattern_node = node.pattern();
        let (pattern, guard) = if let Some(if_node) = pattern_node.as_if_node() {
            let pattern = self.with_pattern(true, |compiler| {
                compiler.visit_statements(if_node.statements())
            });
            let keyword = self.token(if_node.if_keyword_loc())?;
            let predicate = self.visit(&if_node.predicate())?;
            (
                pattern,
                Some(self.builder.guard("if_guard", &keyword, predicate)),
            )
        } else if let Some(unless_node) = pattern_node.as_unless_node() {
            let pattern = self.with_pattern(true, |compiler| {
                compiler.visit_statements(unless_node.statements())
            });
            let keyword = self.token_at(&unless_node.keyword_loc());
            let predicate = self.visit(&unless_node.predicate())?;
            (
                pattern,
                Some(self.builder.guard("unless_guard", &keyword, predicate)),
            )
        } else {
            let pattern = self.with_pattern(true, |compiler| compiler.visit(&pattern_node));
            (pattern, None)
        };
        let in_token = self.token_at(&node.in_loc());
        let then = match node.then_loc() {
            Some(location) => Some(self.token_at(&location)),
            None => {
                let end = node
                    .statements()
                    .map(|statements| statements.location().start_offset());
                self.semicolon(pattern_node.location().end_offset(), end)
            }
        };
        let statements = self.visit_statements(node.statements());
        Some(
            self.builder
                .in_pattern(&in_token, pattern, guard, then.as_ref(), statements),
        )
    }

    fn visit_lambda_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_lambda_node()?;
        let operator = self.token_at(&node.operator_loc());
        let call = self.builder.call_lambda(&operator);
        let opening = self.token_at(&node.opening_loc());
        let arguments = match node.parameters() {
            None => BlockArguments::Arguments(self.builder.args(None, Vec::new(), None)),
            Some(parameters) => {
                if parameters.as_it_parameters_node().is_some() {
                    BlockArguments::It
                } else if let Some(numbered) = parameters.as_numbered_parameters_node() {
                    BlockArguments::Numbered(numbered.maximum())
                } else {
                    let parameters = parameters.as_block_parameters_node()?;
                    let opening = self.token(parameters.opening_loc());
                    let visited = self.visit_block_parameters_node(&parameters);
                    let closing = self.token(parameters.closing_loc());
                    BlockArguments::Arguments(self.builder.args(
                        opening.as_ref(),
                        visited,
                        closing.as_ref(),
                    ))
                }
            }
        };
        let body = self.visit_optional(node.body());
        let closing = self.token_at(&node.closing_loc());
        Some(
            self.builder
                .block(call, &opening, arguments, body, &closing),
        )
    }

    fn visit_block_parameters_node(
        &mut self,
        node: &ruby_prism::BlockParametersNode<'pr>,
    ) -> Vec<Rc<Node>> {
        let mut parameters = match node.parameters() {
            Some(parameters) => self.visit_parameters_node(&parameters),
            None => Vec::new(),
        };
        parameters.extend(self.visit_all(node.locals().iter()));
        parameters
    }

    fn visit_parameter(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        if node.as_required_parameter_node().is_some() {
            self.visit(node)
        } else {
            self.with_destructure(|compiler| compiler.visit(node))
        }
    }

    fn visit_parameters_node(&mut self, node: &ParametersNode<'pr>) -> Vec<Rc<Node>> {
        let mut parameters = Vec::new();
        for required in node.requireds().iter() {
            parameters.extend(self.visit_parameter(&required));
        }
        parameters.extend(self.visit_all(node.optionals().iter()));
        if let Some(rest) = node.rest()
            && rest.as_implicit_rest_node().is_none()
        {
            parameters.extend(self.visit(&rest));
        }
        for post in node.posts().iter() {
            parameters.extend(self.visit_parameter(&post));
        }
        parameters.extend(self.visit_all(node.keywords().iter()));
        if let Some(keyword_rest) = node.keyword_rest() {
            parameters.extend(self.visit(&keyword_rest));
        }
        if let Some(block) = node.block() {
            parameters.extend(self.visit(&block.as_node()));
        }
        parameters
    }

    fn visit_block(&mut self, call: Rc<Node>, block: Option<BlockNode<'pr>>) -> Rc<Node> {
        let Some(block) = block else {
            return call;
        };
        let opening = self.token_at(&block.opening_loc());
        let arguments = match block.parameters() {
            None => BlockArguments::Arguments(self.builder.args(None, Vec::new(), None)),
            Some(parameters) => {
                if parameters.as_it_parameters_node().is_some() {
                    BlockArguments::It
                } else if let Some(numbered) = parameters.as_numbered_parameters_node() {
                    BlockArguments::Numbered(numbered.maximum())
                } else if let Some(parameters) = parameters.as_block_parameters_node() {
                    let opening = self.token(parameters.opening_loc());
                    let inner = parameters.parameters();
                    let visited = if is_procarg0(inner.as_ref()) {
                        let mut visited = Vec::new();
                        if let Some(parameter) = inner.and_then(|inner| inner.requireds().first())
                            && let Some(parameter) = self.visit_parameter(&parameter)
                        {
                            visited.push(self.builder.procarg0(parameter));
                        }
                        visited.extend(self.visit_all(parameters.locals().iter()));
                        visited
                    } else {
                        self.visit_block_parameters_node(&parameters)
                    };
                    let closing = self.token(parameters.closing_loc());
                    BlockArguments::Arguments(self.builder.args(
                        opening.as_ref(),
                        visited,
                        closing.as_ref(),
                    ))
                } else {
                    BlockArguments::Arguments(self.builder.args(None, Vec::new(), None))
                }
            }
        };
        let body = self.visit_optional(block.body());
        let closing = self.token_at(&block.closing_loc());
        self.builder
            .block(call, &opening, arguments, body, &closing)
    }

    fn visit_string_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_string_node()?;
        let opening = node.opening_loc();
        let opening_bytes = opening
            .as_ref()
            .map(|location| self.location_bytes(location));
        let content = node.content_loc();
        if starts_with_any(opening_bytes, &["<<"]) {
            let parts = vec![HeredocPart::Content {
                unescaped: node.unescaped().to_vec(),
                content: self.range(&content),
            }];
            return Some(self.visit_heredoc(
                opening?,
                parts,
                node.closing_loc()?,
                ComposedString::String,
            ));
        }
        if opening_bytes == Some(b"?".as_slice()) {
            let range = self.range(&node.location());
            return Some(self.builder.character(lossy(node.unescaped()), range));
        }
        let opening_token = self.token(opening);
        let closing = self.token(node.closing_loc());
        if starts_with_any(opening_bytes, &["%"]) && node.unescaped().is_empty() {
            return Some(self.builder.string_compose(
                opening_token.as_ref(),
                Vec::new(),
                closing.as_ref(),
            ));
        }
        let content_bytes = self.location_bytes(&content);
        let parts = if content_bytes.contains(&b'\n') {
            self.string_nodes_from_line_continuations(
                node.unescaped(),
                content_bytes,
                content.start_offset(),
                opening_bytes,
            )
        } else {
            let range = self.range(&content);
            vec![self.builder.string_internal(lossy(node.unescaped()), range)]
        };
        Some(
            self.builder
                .string_compose(opening_token.as_ref(), parts, closing.as_ref()),
        )
    }

    fn visit_symbol_node(&mut self, node: &PrismNode<'pr>) -> Option<Rc<Node>> {
        let node = node.as_symbol_node()?;
        let range = self.range(&node.location());
        let Some(closing) = node.closing_loc() else {
            return Some(match node.opening_loc() {
                None => self.builder.symbol_internal(lossy(node.unescaped()), range),
                Some(_) => self.builder.symbol(lossy(node.unescaped()), range),
            });
        };
        let opening = node.opening_loc();
        let opening_bytes = opening
            .as_ref()
            .map(|location| self.location_bytes(location));
        let parts = match node.value_loc() {
            None => Vec::new(),
            Some(value) => {
                let value_bytes = self.location_bytes(&value);
                if value_bytes.is_empty() {
                    Vec::new()
                } else if value_bytes.contains(&b'\n') {
                    self.string_nodes_from_line_continuations(
                        node.unescaped(),
                        value_bytes,
                        value.start_offset(),
                        opening_bytes,
                    )
                } else {
                    let range = self.range(&value);
                    vec![self.builder.string_internal(lossy(node.unescaped()), range)]
                }
            }
        };
        let opening = self.token(opening);
        let closing = self.token_at(&closing);
        Some(
            self.builder
                .symbol_compose(opening.as_ref(), parts, Some(&closing)),
        )
    }

    fn visit_regexp(
        &mut self,
        opening: PrismLocation<'pr>,
        content: PrismLocation<'pr>,
        closing: PrismLocation<'pr>,
        unescaped: &[u8],
    ) -> Rc<Node> {
        let opening_bytes = self.location_bytes(&opening);
        let content_bytes = self.location_bytes(&content);
        let parts = if content_bytes.is_empty() {
            Vec::new()
        } else if content_bytes.contains(&b'\n') {
            self.string_nodes_from_line_continuations(
                unescaped,
                content_bytes,
                content.start_offset(),
                Some(opening_bytes),
            )
        } else {
            let range = self.range(&content);
            vec![self.builder.string_internal(lossy(unescaped), range)]
        };
        self.compose_regexp(&opening, parts, &closing)
    }

    fn visit_interpolated_regexp(
        &mut self,
        opening: PrismLocation<'pr>,
        parts: NodeList<'pr>,
        closing: PrismLocation<'pr>,
    ) -> Rc<Node> {
        let opening_bytes = self.location_bytes(&opening);
        let parts = self.string_nodes_from_interpolation(parts, Some(opening_bytes));
        self.compose_regexp(&opening, parts, &closing)
    }

    fn compose_regexp(
        &mut self,
        opening: &PrismLocation<'pr>,
        parts: Vec<Rc<Node>>,
        closing: &PrismLocation<'pr>,
    ) -> Rc<Node> {
        let opening = self.token_at(opening);
        let closing_start = closing.start_offset();
        let closing_end = closing.end_offset();
        let closing_bytes = self.location_bytes(closing);
        let delimiter_length = lossy(closing_bytes)
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        let delimiter = Token::new(
            lossy(&closing_bytes[..delimiter_length.min(closing_bytes.len())]),
            self.offsets(closing_start, closing_start + 1),
        );
        let options = Token::new(
            lossy(&closing_bytes[delimiter_length.min(closing_bytes.len())..]),
            self.offsets(closing_start + 1, closing_end),
        );
        let options = self.builder.regexp_options(&options);
        self.builder
            .regexp_compose(&opening, parts, &delimiter, options)
    }

    fn string_nodes_from_interpolation(
        &mut self,
        parts: NodeList<'pr>,
        opening: Option<&'pr [u8]>,
    ) -> Vec<Rc<Node>> {
        let mut nodes = Vec::new();
        for part in parts.iter() {
            if let Some(string) = part.as_string_node()
                && string.opening_loc().is_none()
            {
                let content = string.content_loc();
                let content_bytes = self.location_bytes(&content);
                if content_bytes.contains(&b'\n') {
                    nodes.extend(self.string_nodes_from_line_continuations(
                        string.unescaped(),
                        content_bytes,
                        content.start_offset(),
                        opening,
                    ));
                    continue;
                }
            }
            nodes.extend(self.visit(&part));
        }
        nodes
    }

    fn visit_heredoc(
        &mut self,
        opening: PrismLocation<'pr>,
        parts: Vec<HeredocPart<'pr>>,
        closing: PrismLocation<'pr>,
        composed: ComposedString,
    ) -> Rc<Node> {
        let opening_bytes = self.location_bytes(&opening);
        let mut children: Vec<Rc<Node>> = Vec::new();
        let mut indented = false;

        if opening_bytes.starts_with(b"<<~")
            && let Some(HeredocPart::Prism(first)) = parts.first()
            && first.as_string_node().is_none()
        {
            let location = first.location();
            let start = self.line_start(location.start_offset());
            let end = start + (location.end_offset() - location.start_offset());
            let range = self.offsets(start, end);
            children.push(
                self.builder
                    .string_internal(lossy(self.bytes(start, end)), range),
            );
            indented = true;
        }

        for part in parts {
            let string = match &part {
                HeredocPart::Content { unescaped, content } => {
                    Some((unescaped.clone(), *content, content.begin_pos))
                }
                HeredocPart::Prism(node) => node.as_string_node().map(|string| {
                    (
                        string.unescaped().to_vec(),
                        self.range(&string.content_loc()),
                        string.location().start_offset(),
                    )
                }),
            };
            let pushing = match string {
                Some((unescaped, content, start))
                    if self
                        .bytes(content.begin_pos, content.end_pos)
                        .contains(&b'\n') =>
                {
                    let content_bytes = self.bytes(content.begin_pos, content.end_pos);
                    self.string_nodes_from_line_continuations(
                        &unescaped,
                        content_bytes,
                        start,
                        Some(opening_bytes),
                    )
                }
                Some((unescaped, content, _)) if matches!(part, HeredocPart::Content { .. }) => {
                    vec![self.builder.string_internal(lossy(&unescaped), content)]
                }
                _ => match &part {
                    HeredocPart::Prism(node) => self.visit(node).into_iter().collect(),
                    HeredocPart::Content { .. } => Vec::new(),
                },
            };

            for child in pushing {
                let child_value =
                    (child.type_ == "str").then(|| child.child(0).string().map(str::to_string));
                if child.type_ == "str"
                    && matches!(child.children.last(), Some(Child::String(value)) if value.is_empty())
                {
                    continue;
                }
                let appendee_value = children
                    .last()
                    .filter(|last| last.type_ == "str")
                    .and_then(|last| last.child(0).string().map(str::to_string));
                match (child_value, appendee_value) {
                    (Some(child_value), Some(appendee_value))
                        if !appendee_value.ends_with('\n') =>
                    {
                        let appendee = children.pop().expect("the last child exists");
                        let mut location = appendee.location.clone();
                        location.expression =
                            match (appendee.location.expression, child.location.expression) {
                                (Some(left), Some(right)) => Some(left.join(right)),
                                (left, right) => left.or(right),
                            };
                        let value = format!("{appendee_value}{}", child_value.unwrap_or_default());
                        children.push(appendee.updated(
                            Some("str"),
                            Some(vec![Child::String(value)]),
                            Some(location),
                        ));
                    }
                    _ => children.push(child),
                }
            }
        }

        let closing_bytes = self.location_bytes(&closing);
        let closing_text = lossy(closing_bytes);
        let trailing = trailing_space_length(closing_bytes);
        let closing_token = Token::new(
            chomp(&closing_text).to_string(),
            self.offsets(closing.start_offset(), closing.end_offset() - trailing),
        );
        let opening_token = self.token_at(&opening);
        let composed_node = match composed {
            ComposedString::String => self.builder.string_compose(
                Some(&opening_token),
                children.clone(),
                Some(&closing_token),
            ),
            ComposedString::ExecutableString => self.builder.xstring_compose(
                Some(&opening_token),
                children.clone(),
                Some(&closing_token),
            ),
        };
        if indented {
            let rest = children.into_iter().skip(1).map(Child::Node).collect();
            composed_node.updated(None, Some(rest), None)
        } else {
            composed_node
        }
    }

    fn string_nodes_from_line_continuations(
        &mut self,
        unescaped: &[u8],
        escaped: &[u8],
        start_offset: usize,
        opening: Option<&[u8]>,
    ) -> Vec<Rc<Node>> {
        let unescaped = lines(unescaped);
        let escaped = lines(escaped);
        let percent_array = starts_with_any(opening, &["%w", "%W", "%i", "%I"]);
        let regexp = opening == Some(b"/".as_slice()) || starts_with_any(opening, &["%r"]);
        let mut start_offset = start_offset;
        let mut nodes = Vec::new();

        let non_interpolating = opening.is_some_and(|opening| opening.ends_with(b"'"))
            || starts_with_any(opening, &["%q", "%s", "%w", "%i"]);
        if non_interpolating {
            let mut current_length = 0;
            let mut current_line: Vec<u8> = Vec::new();
            for (index, escaped_line) in escaped.iter().enumerate() {
                let unescaped_line = unescaped.get(index).copied().unwrap_or_default();
                current_length += escaped_line.len();
                current_line.extend_from_slice(unescaped_line);
                if percent_array
                    && has_backslash_before_final_newline(escaped_line)
                    && index != escaped.len() - 1
                {
                    continue;
                }
                let range = self.offsets(start_offset, start_offset + current_length);
                nodes.push(self.builder.string_internal(lossy(&current_line), range));
                start_offset += escaped_line.len();
                current_line.clear();
                current_length = 0;
            }
            return nodes;
        }

        let mut chunks: Vec<Vec<&[u8]>> = Vec::new();
        for line in &escaped {
            match chunks.last_mut() {
                Some(chunk)
                    if chunk
                        .last()
                        .is_some_and(|before| has_odd_backslashes_before_final_newline(before)) =>
                {
                    chunk.push(line)
                }
                _ => chunks.push(vec![line]),
            }
        }

        let mut escaped_lengths = Vec::new();
        let mut normalized_lengths: Vec<usize> = Vec::new();
        let mut do_next_tokens: Vec<bool> = Vec::new();
        for chunk in &chunks {
            let chunk_length: usize = chunk.iter().map(|line| line.len()).sum();
            escaped_lengths.push(chunk_length);
            let unescaped_lines_count: usize = if regexp {
                0
            } else {
                chunk
                    .iter()
                    .map(|line| {
                        let mut count = escaped_newline_count(line);
                        if !line.ends_with(b"\n") && count > 0 {
                            count -= 1;
                        }
                        count
                    })
                    .sum()
            };
            let extra = if percent_array { chunk.len() } else { 1 };
            normalized_lengths.extend(std::iter::repeat_n(0, unescaped_lines_count + extra));
            if let Some(last) = normalized_lengths.last_mut() {
                *last = chunk_length;
            }
            do_next_tokens.extend(std::iter::repeat_n(false, unescaped_lines_count + extra));
            if let Some(last) = do_next_tokens.last_mut() {
                *last = true;
            }
        }

        let mut current_line: Vec<u8> = Vec::new();
        let mut current_normalized_length = 0;
        let mut emitted_count = 0;
        for (index, unescaped_line) in unescaped.iter().enumerate() {
            current_line.extend_from_slice(unescaped_line);
            current_normalized_length += normalized_lengths.get(index).copied().unwrap_or(0);
            if do_next_tokens.get(index).copied().unwrap_or(false) {
                let range = self.offsets(start_offset, start_offset + current_normalized_length);
                nodes.push(self.builder.string_internal(lossy(&current_line), range));
                start_offset += escaped_lengths.get(emitted_count).copied().unwrap_or(0);
                current_line.clear();
                current_normalized_length = 0;
                emitted_count += 1;
            }
        }
        nodes
    }
}
