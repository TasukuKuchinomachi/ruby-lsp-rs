use ruby_prism::Node;
use serde::Deserialize;
use serde_json::Value;

use super::code_actions::{
    CREATE_ATTRIBUTE_ACCESSOR, CREATE_ATTRIBUTE_READER, CREATE_ATTRIBUTE_WRITER, EXTRACT_TO_METHOD_TITLE,
    EXTRACT_TO_VARIABLE_TITLE, INSTANCE_VARIABLE_NODES, TOGGLE_BLOCK_STYLE_TITLE,
};
use super::support::workspace_edit::{CodeAction, DocumentChange, TextDocumentEdit, TextEdit, WorkspaceEdit};
use super::{Outgoing, RequestContext, RequestError, RequestResult, parse_params, to_result};
use crate::document::ParsedDocument;
use crate::global_state::message_type;
use crate::position::{Position, Range};
use crate::prism::{NodeExt, child_nodes, constant_id_name, location_slice, node_slice};

pub const NEW_VARIABLE_NAME: &str = "new_variable";
pub const NEW_METHOD_NAME: &str = "new_method";

const INVALID_SELECTION: &str = "Invalid selection for refactor";
const INVALID_TARGET_RANGE: &str = "Couldn't find an appropriate location to place extracted refactor";

#[derive(Debug, Clone, Deserialize)]
struct CodeActionData {
    range: Range,
    uri: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CodeActionResolveParams {
    #[serde(default)]
    title: Option<String>,
    data: CodeActionData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CodeActionError {
    Failed(String),
    Request(RequestError),
}

impl From<RequestError> for CodeActionError {
    fn from(error: RequestError) -> Self {
        CodeActionError::Request(error)
    }
}

impl From<crate::position::InvalidLocationError> for CodeActionError {
    fn from(error: crate::position::InvalidLocationError) -> Self {
        CodeActionError::Request(error.into())
    }
}

fn failure<T>(message: &str) -> Result<T, CodeActionError> {
    Err(CodeActionError::Failed(message.to_string()))
}

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: CodeActionResolveParams = parse_params(params)?;
    let document = context.store.get(&params.data.uri)?;
    if !document.is_ruby() {
        return fail_request_and_notify(context.outgoing, "Code actions are currently only available for Ruby documents");
    }

    let parsed = document.parse();
    let resolver = CodeActionResolve { parsed: &parsed, title: params.title.as_deref(), data: &params.data };
    let outcome = resolver.perform();
    drop(parsed);
    match outcome {
        Ok(code_action) => to_result(code_action),
        Err(CodeActionError::Failed(message)) => fail_request_and_notify(context.outgoing, &message),
        Err(CodeActionError::Request(error)) => Err(error),
    }
}

fn fail_request_and_notify(outgoing: &Outgoing, message: &str) -> RequestResult {
    outgoing.show_message(message, message_type::INFO);
    Err(RequestError::RequestFailed(message.to_string()))
}

struct CodeActionResolve<'a, 'document> {
    parsed: &'a ParsedDocument<'document>,
    title: Option<&'a str>,
    data: &'a CodeActionData,
}

fn leading_spaces(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

fn ruby_strip(text: &str) -> &str {
    text.trim_matches(|character: char| matches!(character, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | '\0'))
}

impl CodeActionResolve<'_, '_> {
    fn source(&self) -> &str {
        self.parsed.document.source()
    }

    fn lines(&self) -> Vec<&str> {
        self.source().split_inclusive('\n').collect()
    }

    fn start_position(&self, node: &Node<'_>) -> Position {
        self.parsed.byte_position(node.start_offset())
    }

    fn end_position(&self, node: &Node<'_>) -> Position {
        self.parsed.byte_position(node.end_offset())
    }

    fn perform(&self) -> Result<CodeAction, CodeActionError> {
        if self.source().is_empty() {
            return failure(INVALID_SELECTION);
        }

        match self.title {
            Some(EXTRACT_TO_VARIABLE_TITLE) => self.refactor_variable(),
            Some(EXTRACT_TO_METHOD_TITLE) => self.refactor_method(),
            Some(TOGGLE_BLOCK_STYLE_TITLE) => self.switch_block_style(),
            Some(CREATE_ATTRIBUTE_READER) | Some(CREATE_ATTRIBUTE_WRITER) | Some(CREATE_ATTRIBUTE_ACCESSOR) => {
                self.create_attribute_accessor()
            }
            title => failure(&format!("Unknown code action: {}", title.unwrap_or(""))),
        }
    }

    fn code_action(&self, title: &str, edits: Vec<TextEdit>) -> CodeAction {
        CodeAction {
            title: title.to_string(),
            kind: None,
            edit: Some(WorkspaceEdit::from_document_changes(vec![DocumentChange::Edit(TextDocumentEdit::new(
                self.data.uri.clone(),
                edits,
            ))])),
            data: None,
        }
    }

    fn switch_block_style(&self) -> Result<CodeAction, CodeActionError> {
        let source_range = self.data.range;
        let (target, node) = if source_range.start == source_range.end {
            let block_context = self.parsed.locate_node(source_range.start, &["BlockNode"])?;
            let Some(node) = block_context.node.filter(|node| node.as_block_node().is_some()) else {
                return failure("Cursor is not inside a block");
            };

            let call_context = self.parsed.locate(node.start_offset(), &["CallNode"]);
            let target = call_context.node.filter(|target| {
                target.as_call_node().and_then(|call| call.block()).is_some_and(|block| block.same_node(&node))
            });
            let Some(target) = target else { return failure(INVALID_TARGET_RANGE) };
            (target, node)
        } else {
            let target = self.parsed.locate_first_within_range(source_range, &["CallNode"])?;
            let Some(target) = target.filter(|target| target.as_call_node().is_some()) else {
                return failure(INVALID_TARGET_RANGE);
            };
            let block = target.as_call_node().and_then(|call| call.block());
            let Some(node) = block.filter(|block| block.as_block_node().is_some()) else {
                return failure(INVALID_TARGET_RANGE);
            };
            (target, node)
        };

        let block = node.as_block_node().expect("checked above");
        let indentation = if location_slice(&block.opening_loc()) == "do" {
            None
        } else {
            Some(" ".repeat(self.start_position(&target).character as usize))
        };

        let new_text = self.recursively_switch_nested_block_styles(&node, indentation.as_deref())?;
        let edit = TextEdit::new(self.parsed.range_from_node(&node), new_text);
        Ok(self.code_action(TOGGLE_BLOCK_STYLE_TITLE, vec![edit]))
    }

    fn refactor_variable(&self) -> Result<CodeAction, CodeActionError> {
        let source_range = self.data.range;
        if source_range.start == source_range.end {
            return failure(INVALID_SELECTION);
        }

        let start_index = self.parsed.offset_of(source_range.start)?;
        let end_index = self.parsed.offset_of(source_range.end)?;
        let extracted_source = self.source().get(start_index..end_index).unwrap_or("");

        let node_context = self.parsed.locate(start_index, &["StatementsNode", "BlockNode"]);
        let Some(closest_statements) = node_context.node.as_ref() else { return failure(INVALID_TARGET_RANGE) };
        let statements_children = child_nodes(closest_statements);
        if statements_children.is_empty() {
            return failure(INVALID_TARGET_RANGE);
        }

        let start_line = i64::from(source_range.start.line);
        let mut closest_node = &statements_children[0];
        let mut closest_distance = f64::INFINITY;
        for (position, node) in statements_children.iter().enumerate() {
            let distance = start_line - i64::from(self.end_position(node).line);
            let distance = if distance <= 0 { f64::INFINITY } else { distance as f64 };
            if position == 0 || distance < closest_distance {
                closest_node = node;
                closest_distance = distance;
            }
        }

        if closest_node.as_missing_node().is_some() {
            return failure(INVALID_TARGET_RANGE);
        }

        let closest_start_line = self.start_position(closest_node).line;
        let closest_end_line = self.end_position(closest_node).line;
        let parent_is_one_line_block = node_context.parent.as_ref().is_some_and(|parent| {
            parent.as_block_node().is_some() && self.start_position(parent).line == self.end_position(parent).line
        });

        let (target_range, variable_source) = if parent_is_one_line_block {
            let character = source_range.start.character.saturating_sub(1);
            let position = Position::new(closest_end_line, character);
            (Range::new(position, position), format!(" {NEW_VARIABLE_NAME} = {extracted_source};"))
        } else {
            let (indentation_line_number, target_line) =
                if closest_start_line <= source_range.start.line && closest_end_line >= source_range.end.line {
                    (closest_start_line, closest_start_line)
                } else {
                    (closest_end_line, closest_end_line + 1)
                };

            let lines = self.lines();
            let Some(indentation_line) = lines.get(indentation_line_number as usize) else {
                return failure(INVALID_TARGET_RANGE);
            };
            let indentation = leading_spaces(indentation_line);
            let position = Position::new(target_line, indentation as u32);

            let Some(line) = lines.get(target_line as usize) else { return failure(INVALID_TARGET_RANGE) };
            let indents = " ".repeat(indentation);
            let variable_source = if ruby_strip(line).is_empty() {
                format!("\n{indents}{NEW_VARIABLE_NAME} = {extracted_source}")
            } else {
                format!("{NEW_VARIABLE_NAME} = {extracted_source}\n{indents}")
            };
            (Range::new(position, position), variable_source)
        };

        Ok(self.code_action(
            EXTRACT_TO_VARIABLE_TITLE,
            vec![TextEdit::new(source_range, NEW_VARIABLE_NAME), TextEdit::new(target_range, variable_source)],
        ))
    }

    fn refactor_method(&self) -> Result<CodeAction, CodeActionError> {
        let source_range = self.data.range;
        if source_range.start == source_range.end {
            return failure(INVALID_SELECTION);
        }

        let start_index = self.parsed.offset_of(source_range.start)?;
        let end_index = self.parsed.offset_of(source_range.end)?;
        let extracted_source = self.source().get(start_index..end_index).unwrap_or("");

        let node_context = self.parsed.locate(start_index, &["DefNode"]);
        let Some(closest_node) = node_context.node.as_ref() else { return failure(INVALID_TARGET_RANGE) };

        let (target_range, new_method_source) = if let Some(definition) = closest_node.as_def_node() {
            let Some(end_keyword_location) = definition.end_keyword_loc() else { return failure(INVALID_TARGET_RANGE) };
            let end = self.parsed.byte_position(end_keyword_location.end_offset());
            let indentation = " ".repeat(self.parsed.byte_position(end_keyword_location.start_offset()).character as usize);
            let new_method_source = format!(
                "\n\n{indentation}def {NEW_METHOD_NAME}\n{indentation}  {extracted_source}\n{indentation}end"
            );
            (Range::new(end, end), new_method_source)
        } else {
            let new_method_source =
                format!("def {NEW_METHOD_NAME}\n  {}\nend\n\n", extracted_source.replace('\n', "\n  "));
            let position = Position::new(source_range.start.line.saturating_sub(1), source_range.start.character);
            (Range::new(position, position), new_method_source)
        };

        Ok(self.code_action(
            EXTRACT_TO_METHOD_TITLE,
            vec![TextEdit::new(target_range, new_method_source), TextEdit::new(source_range, NEW_METHOD_NAME)],
        ))
    }

    fn recursively_switch_nested_block_styles(
        &self,
        node: &Node<'_>,
        indentation: Option<&str>,
    ) -> Result<String, CodeActionError> {
        let block = node.as_block_node().expect("only blocks are switched");
        let parameters = block.parameters();
        let body = block.body();

        let mut source = String::new();
        match indentation {
            Some(indentation) => {
                source.push_str("do");
                if let Some(parameters) = &parameters {
                    source.push_str(&format!(" {}", node_slice(parameters)));
                }
                source.push_str(&format!("\n{indentation}  "));
                if let Some(body) = &body {
                    source.push_str(&self.switch_block_body(body, Some(indentation))?);
                }
                source.push_str(&format!("\n{indentation}end"));
            }
            None => {
                source.push_str("{ ");
                if let Some(parameters) = &parameters {
                    source.push_str(&format!("{} ", node_slice(parameters)));
                }
                if let Some(body) = &body {
                    source.push_str(&self.switch_block_body(body, None)?);
                }
                source.push('}');
            }
        }
        Ok(source)
    }

    fn switch_block_body(&self, body: &Node<'_>, indentation: Option<&str>) -> Result<String, CodeActionError> {
        let body_range = self.parsed.range_from_node(body);
        let nested_block = self.parsed.locate_first_within_range(body_range, &["BlockNode"])?;

        let mut body_content = node_slice(body);
        if let Some(nested_block) = nested_block.filter(|nested_block| nested_block.as_block_node().is_some()) {
            let correction_start = nested_block.start_offset() - body.start_offset();
            let correction_end = nested_block.end_offset() - body.start_offset();
            let next_indentation = indentation.map(|indentation| format!("{indentation}  "));
            let replacement = self.recursively_switch_nested_block_styles(&nested_block, next_indentation.as_deref())?;
            body_content.replace_range(correction_start..correction_end, &replacement);
        }

        Ok(match indentation {
            Some(_) => body_content.replace(';', "\n"),
            None => format!("{} ", body_content.replace('\n', ";")),
        })
    }

    fn create_attribute_accessor(&self) -> Result<CodeAction, CodeActionError> {
        let source_range = self.data.range;

        let mut node = if source_range.start != source_range.end {
            self.parsed.locate_first_within_range(source_range, INSTANCE_VARIABLE_NODES)?
        } else {
            None
        };

        if node.is_none() {
            let node_context = self.parsed.locate_node(source_range.start, INSTANCE_VARIABLE_NODES)?;
            node = node_context.node.filter(|node| INSTANCE_VARIABLE_NODES.contains(&node.type_name()));
            if node.is_none() {
                return failure(INVALID_SELECTION);
            }
        }
        let node = node.expect("checked above");

        let start = self.start_position(&node);
        let line_start = self.parsed.line_index.line_start(start.line as usize).unwrap_or(0);
        let start_character_column = self.source()[line_start..node.start_offset()].chars().count() as u32;
        let node_context = self.parsed.locate_node(
            Position::new(start.line + 1, start_character_column),
            &["ClassNode", "ModuleNode", "SingletonClassNode"],
        )?;
        let Some(closest_node) = node_context.node.as_ref() else { return failure(INVALID_TARGET_RANGE) };

        let attribute_name = instance_variable_name(&node).unwrap_or_default();
        let attribute_name = attribute_name.strip_prefix('@').unwrap_or(&attribute_name);
        let closest_start = self.start_position(closest_node);
        let indentation = " ".repeat(closest_start.character as usize + 2);
        let title = self.title.unwrap_or_default();
        let method = match title {
            CREATE_ATTRIBUTE_READER => "attr_reader",
            CREATE_ATTRIBUTE_WRITER => "attr_writer",
            _ => "attr_accessor",
        };
        let attribute_accessor_source = format!("{indentation}{method} :{attribute_name}\n\n");

        let target_position = Position::new(closest_start.line + 1, 0);
        Ok(self.code_action(
            title,
            vec![TextEdit::new(Range::new(target_position, target_position), attribute_accessor_source)],
        ))
    }
}

fn instance_variable_name(node: &Node<'_>) -> Option<String> {
    let name = match node {
        Node::InstanceVariableAndWriteNode { .. } => node.as_instance_variable_and_write_node()?.name(),
        Node::InstanceVariableOperatorWriteNode { .. } => node.as_instance_variable_operator_write_node()?.name(),
        Node::InstanceVariableOrWriteNode { .. } => node.as_instance_variable_or_write_node()?.name(),
        Node::InstanceVariableReadNode { .. } => node.as_instance_variable_read_node()?.name(),
        Node::InstanceVariableTargetNode { .. } => node.as_instance_variable_target_node()?.name(),
        Node::InstanceVariableWriteNode { .. } => node.as_instance_variable_write_node()?.name(),
        _ => return None,
    };
    Some(constant_id_name(&name))
}
