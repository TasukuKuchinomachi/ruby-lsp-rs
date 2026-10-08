use ruby_prism::{CallNode, Node};

use crate::indexer::Index;
use crate::node_context::NodeContext;
use crate::prism::{NodeExt, constant_name, location_slice, node_slice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Type {
    pub name: String,
    pub guessed: bool,
}

impl Type {
    pub fn known(name: impl Into<String>) -> Type {
        Type { name: name.into(), guessed: false }
    }

    pub fn guessed(name: impl Into<String>) -> Type {
        Type { name: name.into(), guessed: true }
    }

    pub fn attached(&self) -> Type {
        let parts: Vec<&str> = split_name(&self.name);
        let attached = if parts.is_empty() { String::new() } else { parts[..parts.len() - 1].join("::") };
        Type::known(attached)
    }
}

fn split_name(name: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = name.split("::").collect();
    while parts.last() == Some(&"") {
        parts.pop();
    }
    parts
}

pub struct TypeInferrer<'index> {
    index: &'index mut Index,
}

impl<'index> TypeInferrer<'index> {
    pub fn new(index: &'index mut Index) -> TypeInferrer<'index> {
        TypeInferrer { index }
    }

    pub fn infer_receiver_type(&mut self, node_context: &NodeContext<'_>) -> Option<Type> {
        let node = node_context.node.as_ref()?;
        match node.type_name() {
            "CallNode" => self.infer_receiver_for_call_node(&node.as_call_node()?, node_context),
            "InstanceVariableReadNode"
            | "InstanceVariableAndWriteNode"
            | "InstanceVariableWriteNode"
            | "InstanceVariableOperatorWriteNode"
            | "InstanceVariableOrWriteNode"
            | "InstanceVariableTargetNode"
            | "SuperNode"
            | "ForwardingSuperNode" => Some(self_receiver_handling(node_context)),
            "ClassVariableAndWriteNode"
            | "ClassVariableWriteNode"
            | "ClassVariableOperatorWriteNode"
            | "ClassVariableOrWriteNode"
            | "ClassVariableReadNode"
            | "ClassVariableTargetNode" => self.infer_receiver_for_class_variables(node_context),
            _ => None,
        }
    }

    fn infer_receiver_for_call_node(&mut self, node: &CallNode<'_>, node_context: &NodeContext<'_>) -> Option<Type> {
        let mut receiver = node.receiver();

        if let Some(parentheses) = receiver.as_ref().and_then(|receiver| receiver.as_parentheses_node())
            && let Some(statements) = parentheses.body().and_then(|body| body.as_statements_node())
        {
            let body: Vec<Node<'_>> = statements.body().iter().collect();
            if body.len() == 1 {
                receiver = body.into_iter().next();
            }
        }

        let Some(receiver) = receiver else { return Some(self_receiver_handling(node_context)) };

        match receiver.type_name() {
            "SelfNode" => Some(self_receiver_handling(node_context)),
            "StringNode" => Some(Type::known("String")),
            "SymbolNode" => Some(Type::known("Symbol")),
            "ArrayNode" => Some(Type::known("Array")),
            "HashNode" => Some(Type::known("Hash")),
            "IntegerNode" => Some(Type::known("Integer")),
            "FloatNode" => Some(Type::known("Float")),
            "RegularExpressionNode" => Some(Type::known("Regexp")),
            "NilNode" => Some(Type::known("NilClass")),
            "TrueNode" => Some(Type::known("TrueClass")),
            "FalseNode" => Some(Type::known("FalseClass")),
            "RangeNode" => Some(Type::known("Range")),
            "LambdaNode" => Some(Type::known("Proc")),
            "ConstantPathNode" | "ConstantReadNode" => {
                let receiver_name = constant_name(&receiver)?;
                let resolved = self.index.resolve(&receiver_name, &node_context.nesting)?;
                let name = self.index.entry(*resolved.first()?).name.clone();
                let mut parts = split_name(&name);
                let last = parts.pop()?;
                if parts.is_empty() {
                    Some(Type::known(format!("{last}::<Class:{last}>")))
                } else {
                    Some(Type::known(format!("{}::{last}::<Class:{last}>", parts.join("::"))))
                }
            }
            "CallNode" => {
                let receiver_call = receiver.as_call_node()?;
                let raw_receiver = receiver_call.message_loc().map(|location| location_slice(&location))?;
                if raw_receiver == "new" {
                    let receiver_type = self.infer_receiver_for_call_node(&receiver_call, node_context)?;
                    let new_method = self.index.resolve_method("new", &receiver_type.name, false).and_then(|ids| ids.first().copied());
                    if let Some(new_method) = new_method
                        && self.index.entry(new_method).owner() != Some("Class")
                    {
                        return None;
                    }
                    Some(receiver_type.attached())
                } else {
                    self.guess_type(&raw_receiver, &node_context.nesting)
                }
            }
            _ => self.guess_type(&node_slice(&receiver), &node_context.nesting),
        }
    }

    fn guess_type(&mut self, raw_receiver: &str, nesting: &[String]) -> Option<Type> {
        let without_prefix = raw_receiver.strip_prefix('@').unwrap_or(raw_receiver);
        let without_prefix = without_prefix.strip_prefix("@@").unwrap_or(without_prefix);
        let guessed_name: String = without_prefix.split('_').map(capitalize).collect();

        let entries = self.index.resolve(&guessed_name, nesting).or_else(|| self.index.first_unqualified_const(&guessed_name))?;
        let name = self.index.entry(*entries.first()?).name.clone();
        Some(Type::guessed(name))
    }

    fn infer_receiver_for_class_variables(&mut self, node_context: &NodeContext<'_>) -> Option<Type> {
        let mut nesting_parts = node_context.nesting.clone();
        if nesting_parts.is_empty() {
            return Some(Type::known("Object"));
        }

        while nesting_parts.last().is_some_and(|part| part.contains("<Class:")) {
            nesting_parts.pop();
        }

        let receiver_name = nesting_parts.join("::");
        let resolved = self.index.resolve(&receiver_name, &node_context.nesting)?;
        let name = self.index.entry(*resolved.first()?).name.clone();
        Some(Type::known(name))
    }
}

fn capitalize(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters.flat_map(char::to_lowercase)).collect(),
        None => String::new(),
    }
}

fn self_receiver_handling(node_context: &NodeContext<'_>) -> Type {
    let nesting = &node_context.nesting;
    if nesting.is_empty() {
        return Type::known("Object");
    }
    if node_context.surrounding_method.is_some() {
        return Type::known(node_context.fully_qualified_name());
    }

    let parts: Vec<&str> = nesting.iter().flat_map(|part| part.split("::")).collect();
    Type::known(format!("{}::<Class:{}>", parts.join("::"), parts.last().copied().unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_context::locate;

    fn infer(source: &str, offset: usize) -> Option<Type> {
        let mut index = Index::default();
        let parse_result = ruby_prism::parse(source.as_bytes());
        let root = parse_result.node();
        let node_context = locate(&root, offset, &["CallNode", "InstanceVariableReadNode", "ClassVariableReadNode", "SuperNode"]);
        TypeInferrer::new(&mut index).infer_receiver_type(&node_context)
    }

    #[test]
    fn infers_receiver_types_of_literals_and_self() {
        assert_eq!(infer("\"hello\".upcase", 9), Some(Type::known("String")));
        assert_eq!(infer("[].push", 4), Some(Type::known("Array")));
        assert_eq!(infer("(0...2).each", 9), Some(Type::known("Range")));
        assert_eq!(infer("foo", 1), Some(Type::known("Object")));

        let source = "class Foo\n  def bar\n    baz\n  end\nend\n";
        assert_eq!(infer(source, source.find("baz").unwrap()), Some(Type::known("Foo")));

        let source = "module Foo::Bar\n  baz\nend\n";
        assert_eq!(infer(source, source.find("baz").unwrap()), Some(Type::known("Foo::Bar::<Class:Bar>")));

        let source = "class Foo\n  def self.bar\n    @a\n  end\nend\n";
        assert_eq!(infer(source, source.find("@a").unwrap()), Some(Type::known("Foo::<Class:Foo>")));
    }

    #[test]
    fn unknown_receivers_without_matching_constants_are_not_guessed() {
        assert_eq!(infer("user.name", 6), None);
    }

    #[test]
    fn capitalizes_like_ruby() {
        assert_eq!(capitalize("fooBar"), "Foobar");
        assert_eq!(capitalize(""), "");
        assert_eq!("user_name".split('_').map(capitalize).collect::<String>(), "UserName");
    }
}
