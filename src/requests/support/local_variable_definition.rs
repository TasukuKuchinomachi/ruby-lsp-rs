use prism_child_nodes::child_nodes;
use ruby_prism::{Location, Node};

use crate::prism::{NodeExt, constant_id_name};

const SCOPE_BOUNDARIES: &[&str] = &["ProgramNode", "DefNode", "ClassNode", "ModuleNode", "SingletonClassNode"];
const NESTED_SCOPES: &[&str] = &["BlockNode", "LambdaNode"];

struct LocalVariable {
    name: String,
    /// Parameters have no depth in Prism since they are always declared in their own scope
    depth: u32,
    start: usize,
    end: usize,
    declares: bool,
}

/// Prism's depth is followed instead of matching names, since a block parameter or a block local variable can shadow a
/// variable of the outer scope with the same name
pub fn definition_range(root: &Node<'_>, offset: usize) -> Option<(usize, usize)> {
    let mut scopes = vec![root.clone_node()];
    let reference = find_reference(root, offset, &mut scopes)?;
    let defining_scope = scopes.get(scopes.len().checked_sub(reference.depth as usize + 1)?)?;
    let mut declarations = Vec::new();
    for child in child_nodes(defining_scope) {
        collect_declarations(&child, &reference.name, 0, &mut declarations);
    }
    declarations.into_iter().min()
}

fn find_reference<'pr>(node: &Node<'pr>, offset: usize, scopes: &mut Vec<Node<'pr>>) -> Option<LocalVariable> {
    for child in child_nodes(node) {
        if offset < child.start_offset() || child.end_offset() < offset {
            continue;
        }
        if let Some(variable) = local_variable(&child)
            && variable.start <= offset
            && offset <= variable.end
        {
            return Some(variable);
        }
        let is_scope = NESTED_SCOPES.contains(&child.type_name()) || SCOPE_BOUNDARIES.contains(&child.type_name());
        if is_scope {
            scopes.push(child.clone_node());
        }
        if let Some(found) = find_reference(&child, offset, scopes) {
            return Some(found);
        }
        if is_scope {
            scopes.pop();
        }
    }
    None
}

fn collect_declarations(node: &Node<'_>, name: &str, depth: u32, declarations: &mut Vec<(usize, usize)>) {
    if let Some(variable) = local_variable(node)
        && variable.declares
        && variable.depth == depth
        && variable.name == name
    {
        declarations.push((variable.start, variable.end));
    }
    let type_name = node.type_name();
    if SCOPE_BOUNDARIES.contains(&type_name) {
        return;
    }
    let child_depth = if NESTED_SCOPES.contains(&type_name) { depth + 1 } else { depth };
    for child in child_nodes(node) {
        collect_declarations(&child, name, child_depth, declarations);
    }
}

fn local_variable(node: &Node<'_>) -> Option<LocalVariable> {
    let variable = |name, depth, location: Location<'_>, declares| LocalVariable {
        name: constant_id_name(&name),
        depth,
        start: location.start_offset(),
        end: location.end_offset(),
        declares,
    };
    let parameter = |name, location: Location<'_>| variable(name, 0, location, true);
    Some(match node {
        Node::LocalVariableReadNode { .. } => {
            let read = node.as_local_variable_read_node()?;
            variable(read.name(), read.depth(), read.location(), false)
        }
        Node::LocalVariableWriteNode { .. } => {
            let write = node.as_local_variable_write_node()?;
            variable(write.name(), write.depth(), write.name_loc(), true)
        }
        Node::LocalVariableAndWriteNode { .. } => {
            let write = node.as_local_variable_and_write_node()?;
            variable(write.name(), write.depth(), write.name_loc(), true)
        }
        Node::LocalVariableOrWriteNode { .. } => {
            let write = node.as_local_variable_or_write_node()?;
            variable(write.name(), write.depth(), write.name_loc(), true)
        }
        Node::LocalVariableOperatorWriteNode { .. } => {
            let write = node.as_local_variable_operator_write_node()?;
            variable(write.name(), write.depth(), write.name_loc(), true)
        }
        Node::LocalVariableTargetNode { .. } => {
            let target = node.as_local_variable_target_node()?;
            variable(target.name(), target.depth(), target.location(), true)
        }
        Node::RequiredParameterNode { .. } => {
            let required = node.as_required_parameter_node()?;
            parameter(required.name(), required.location())
        }
        Node::OptionalParameterNode { .. } => {
            let optional = node.as_optional_parameter_node()?;
            parameter(optional.name(), optional.name_loc())
        }
        Node::RequiredKeywordParameterNode { .. } => {
            let keyword = node.as_required_keyword_parameter_node()?;
            without_colon(parameter(keyword.name(), keyword.name_loc()))
        }
        Node::OptionalKeywordParameterNode { .. } => {
            let keyword = node.as_optional_keyword_parameter_node()?;
            without_colon(parameter(keyword.name(), keyword.name_loc()))
        }
        Node::RestParameterNode { .. } => {
            let rest = node.as_rest_parameter_node()?;
            parameter(rest.name()?, rest.name_loc()?)
        }
        Node::KeywordRestParameterNode { .. } => {
            let keyword_rest = node.as_keyword_rest_parameter_node()?;
            parameter(keyword_rest.name()?, keyword_rest.name_loc()?)
        }
        Node::BlockParameterNode { .. } => {
            let block = node.as_block_parameter_node()?;
            parameter(block.name()?, block.name_loc()?)
        }
        Node::BlockLocalVariableNode { .. } => {
            let block_local = node.as_block_local_variable_node()?;
            parameter(block_local.name(), block_local.location())
        }
        _ => return None,
    })
}

/// Prism's name location of a keyword parameter includes the colon
fn without_colon(variable: LocalVariable) -> LocalVariable {
    let end = variable.start + variable.name.len();
    LocalVariable { end, ..variable }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition_of(source: &str, reference: &str, occurrence: usize) -> Option<usize> {
        let parse_result = ruby_prism::parse(source.as_bytes());
        let offset = source.match_indices(reference).nth(occurrence)?.0;
        definition_range(&parse_result.node(), offset).map(|(start, _)| start)
    }

    #[test]
    fn tells_shadowing_block_parameters_apart_from_outer_variables() {
        let source = "item = 1\n[2].each { |item| item }\nitem\n";
        assert_eq!(definition_of(source, "item", 2), source.find("|item|").map(|offset| offset + 1));
        assert_eq!(definition_of(source, "item", 3), Some(0));
    }

    #[test]
    fn finds_the_first_assignment_of_an_outer_variable_from_a_nested_block() {
        let source = "def total(rows)\n  sum = 0\n  rows.each { |row| [row].each { sum += row } }\n  sum\nend\n";
        let first_assignment = source.find("sum = 0");
        assert_eq!(definition_of(source, "sum", 1), first_assignment);
        assert_eq!(definition_of(source, "sum", 2), first_assignment);
        assert_eq!(definition_of(source, "rows", 1), source.find("rows)"));
    }

    #[test]
    fn ignores_variables_of_other_methods() {
        let source = "def first\n  value = 1\nend\n\ndef second(value)\n  value\nend\n";
        assert_eq!(definition_of(source, "value", 2), source.find("value)"));
    }
}
