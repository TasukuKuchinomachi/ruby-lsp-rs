use std::process::Command;

use ruby_prism::{CallNode, Node};
use serde::Deserialize;

use crate::global_state::GlobalState;
use crate::indexer::{EntryId, Index};
use crate::listeners::definition::variable_name;
use crate::node_context::NodeContext;
use crate::prism::{NodeExt, constant_id_name, constant_name, location_slice};
use crate::requests::support::common::{categorized_markdown_from_index_entries, self_receiver};
use crate::requests::support::hover_response_builder::{HoverCategory, HoverResponseBuilder};
use crate::requests::support::sorbet_level::SorbetLevel;
use crate::requests::support::static_docs::{GUESSED_TYPES_URL, keyword_documentation, keyword_documentation_uri};
use crate::type_inferrer::TypeInferrer;

pub const ALLOWED_TARGETS: &[&str] = &[
    "BreakNode",
    "CallNode",
    "ConstantReadNode",
    "ConstantWriteNode",
    "ConstantPathNode",
    "GlobalVariableAndWriteNode",
    "GlobalVariableOperatorWriteNode",
    "GlobalVariableOrWriteNode",
    "GlobalVariableReadNode",
    "GlobalVariableTargetNode",
    "GlobalVariableWriteNode",
    "InstanceVariableReadNode",
    "InstanceVariableAndWriteNode",
    "InstanceVariableOperatorWriteNode",
    "InstanceVariableOrWriteNode",
    "InstanceVariableTargetNode",
    "InstanceVariableWriteNode",
    "SymbolNode",
    "StringNode",
    "InterpolatedStringNode",
    "SuperNode",
    "ForwardingSuperNode",
    "YieldNode",
    "ClassVariableAndWriteNode",
    "ClassVariableOperatorWriteNode",
    "ClassVariableOrWriteNode",
    "ClassVariableReadNode",
    "ClassVariableTargetNode",
    "ClassVariableWriteNode",
];

const ALLOWED_REMOTE_PROVIDERS: &[&str] = &["https://github.com", "https://gitlab.com"];

pub const GEMFILE_NAME: &str = "Gemfile";

pub struct Hover<'a, 'pr> {
    response_builder: &'a mut HoverResponseBuilder,
    global_state: &'a GlobalState,
    index: &'a mut Index,
    path: Option<String>,
    node_context: &'a NodeContext<'pr>,
    sorbet_level: SorbetLevel,
}

impl<'a, 'pr> Hover<'a, 'pr> {
    pub fn new(
        response_builder: &'a mut HoverResponseBuilder,
        global_state: &'a GlobalState,
        index: &'a mut Index,
        uri: &str,
        node_context: &'a NodeContext<'pr>,
        sorbet_level: SorbetLevel,
    ) -> Hover<'a, 'pr> {
        Hover { response_builder, global_state, index, path: crate::uri::to_path(uri), node_context, sorbet_level }
    }

    pub fn dispatch(&mut self, target: &Node<'_>) {
        match target.type_name() {
            "BreakNode" => {
                let keyword = location_slice(&target.as_break_node().expect("type name was checked").keyword_loc());
                self.handle_keyword_documentation(&keyword);
            }
            "YieldNode" => {
                let keyword = location_slice(&target.as_yield_node().expect("type name was checked").keyword_loc());
                self.handle_keyword_documentation(&keyword);
            }
            "StringNode" => self.on_string_node_enter(target),
            "InterpolatedStringNode" => {
                let opening = target.as_interpolated_string_node().expect("type name was checked").opening_loc();
                self.generate_heredoc_hover(opening.map(|location| location_slice(&location)));
            }
            "ConstantReadNode" | "ConstantPathNode" => {
                if !self.sorbet_level.is_ignore() {
                    return;
                }
                if let Some(name) = constant_name(target) {
                    self.generate_hover(&name);
                }
            }
            "ConstantWriteNode" => {
                if !self.sorbet_level.is_ignore() {
                    return;
                }
                let name = constant_id_name(&target.as_constant_write_node().expect("type name was checked").name());
                self.generate_hover(&name);
            }
            "CallNode" => self.on_call_node_enter(&target.as_call_node().expect("type name was checked")),
            "GlobalVariableAndWriteNode"
            | "GlobalVariableOperatorWriteNode"
            | "GlobalVariableOrWriteNode"
            | "GlobalVariableReadNode"
            | "GlobalVariableTargetNode"
            | "GlobalVariableWriteNode" => {
                if let Some(name) = variable_name(target) {
                    self.handle_global_variable_hover(&name);
                }
            }
            "InstanceVariableReadNode"
            | "InstanceVariableWriteNode"
            | "InstanceVariableAndWriteNode"
            | "InstanceVariableOperatorWriteNode"
            | "InstanceVariableOrWriteNode"
            | "InstanceVariableTargetNode" => {
                if let Some(name) = variable_name(target) {
                    self.handle_instance_variable_hover(&name);
                }
            }
            "SuperNode" | "ForwardingSuperNode" => self.handle_super_node_hover(),
            "ClassVariableAndWriteNode"
            | "ClassVariableOperatorWriteNode"
            | "ClassVariableOrWriteNode"
            | "ClassVariableTargetNode"
            | "ClassVariableReadNode"
            | "ClassVariableWriteNode" => {
                if let Some(name) = variable_name(target) {
                    self.handle_class_variable_hover(&name);
                }
            }
            _ => {}
        }
    }

    fn on_string_node_enter(&mut self, node: &Node<'_>) {
        let string = node.as_string_node().expect("type name was checked");
        let is_gemfile = self
            .path
            .as_deref()
            .and_then(|path| std::path::Path::new(path).file_name())
            .is_some_and(|file_name| file_name == GEMFILE_NAME);

        if is_gemfile && let Some(call_node) = self.node_context.call_node() {
            let first_argument = call_node.arguments().and_then(|arguments| arguments.arguments().iter().next());
            if constant_id_name(&call_node.name()) == "gem" && first_argument.is_some_and(|argument| argument.same_node(node)) {
                self.generate_gem_hover(&call_node);
                return;
            }
        }

        self.generate_heredoc_hover(string.opening_loc().map(|location| location_slice(&location)));
    }

    fn on_call_node_enter(&mut self, node: &CallNode<'_>) {
        if self.sorbet_level.is_true_or_higher() && self_receiver(node) {
            return;
        }
        let Some(message) = node.message_loc().map(|location| location_slice(&location)) else { return };
        self.handle_method_hover(&message, false);
    }

    fn generate_heredoc_hover(&mut self, opening_content: Option<String>) {
        let Some(opening_content) = opening_content else { return };
        if !opening_content.starts_with("<<") {
            return;
        }
        let Some((heredoc_type, delimiter)) = match_heredoc_opening(&opening_content) else { return };

        let message = if heredoc_type == Some('~') {
            format!(
                "This is a squiggly heredoc definition using the `{delimiter}` delimiter. Indentation will be ignored in the resulting string."
            )
        } else {
            format!(
                "This is a heredoc definition using the `{delimiter}` delimiter. Indentation will be considered part of the string."
            )
        };
        self.response_builder.push(&message, HoverCategory::Documentation);
    }

    fn handle_keyword_documentation(&mut self, keyword: &str) {
        let Some(content) = keyword_documentation(keyword) else { return };
        let documentation_uri = keyword_documentation_uri(keyword);
        self.response_builder.push(&format!("```ruby\n{keyword}\n```"), HoverCategory::Title);
        self.response_builder.push(&format!("[Read more]({documentation_uri})"), HoverCategory::Links);
        self.response_builder.push(content, HoverCategory::Documentation);
    }

    fn handle_super_node_hover(&mut self) {
        if self.sorbet_level.is_true_or_higher() {
            return;
        }
        let Some(surrounding_method) = self.node_context.surrounding_method.clone() else { return };
        self.handle_method_hover(&surrounding_method, true);
    }

    fn handle_method_hover(&mut self, message: &str, inherited_only: bool) {
        let Some(receiver_type) = TypeInferrer::new(self.index).infer_receiver_type(self.node_context) else { return };
        let Some(methods) = self.index.resolve_method(message, &receiver_type.name, inherited_only) else { return };
        let Some(first_method) = methods.first().map(|id| self.index.entry(*id)) else { return };

        let mut title = format!("{message}{}", first_method.decorated_parameters());
        title.push_str(&first_method.formatted_signatures());

        if receiver_type.guessed {
            title.push_str(&format!("\n\nGuessed receiver: {}", receiver_type.name));
            self.response_builder.push(&format!("[Learn more about guessed types]({GUESSED_TYPES_URL})\n"), HoverCategory::Links);
        }

        self.push_entries(&title, &methods);
    }

    fn handle_instance_variable_hover(&mut self, name: &str) {
        if self.sorbet_level.is_strict() {
            return;
        }
        let Some(receiver_type) = TypeInferrer::new(self.index).infer_receiver_type(self.node_context) else { return };
        let Some(entries) = self.index.resolve_instance_variable(name, &receiver_type.name) else { return };
        self.push_entries(name, &entries);
    }

    fn handle_global_variable_hover(&mut self, name: &str) {
        let Some(entries) = self.index.get_owned(name) else { return };
        self.push_entries(name, &entries);
    }

    fn handle_class_variable_hover(&mut self, name: &str) {
        let Some(receiver_type) = TypeInferrer::new(self.index).infer_receiver_type(self.node_context) else { return };
        let Some(entries) = self.index.resolve_class_variable(name, &receiver_type.name) else { return };
        self.push_entries(name, &entries);
    }

    fn generate_hover(&mut self, name: &str) {
        let Some(entries) = self.index.resolve(name, &self.node_context.nesting) else { return };
        let Some(first_entry) = entries.first().map(|id| self.index.entry(*id)) else { return };

        let full_name = first_entry.name.clone();
        if first_entry.is_private() && full_name != format!("{}::{name}", self.node_context.fully_qualified_name()) {
            return;
        }

        self.push_entries(&full_name, &entries);
    }

    fn push_entries(&mut self, title: &str, entries: &[EntryId]) {
        let markdown = categorized_markdown_from_index_entries(self.index, title, entries, None);
        self.response_builder.push_categorized(&markdown);
    }

    fn generate_gem_hover(&mut self, node: &CallNode<'_>) {
        let Some(first_argument) = node.arguments().and_then(|arguments| arguments.arguments().iter().next()) else {
            return;
        };
        let Some(string) = first_argument.as_string_node() else { return };
        let Some(specification) = find_gem_specification(self.global_state, &location_slice(&string.content_loc())) else {
            return;
        };

        let information = [specification.description.as_deref(), specification.summary.as_deref()]
            .into_iter()
            .flatten()
            .find(|text| !text.is_empty())
            .unwrap_or("This rubygem does not have a description or summary.");
        let information = remove_leading_spaces_of_each_line(information);

        let remote_url = [specification.homepage.as_deref(), specification.source_code_uri.as_deref()]
            .into_iter()
            .flatten()
            .find(|page| ALLOWED_REMOTE_PROVIDERS.iter().any(|provider| page.starts_with(provider)));
        let remote_link = remote_url.map(|url| format!(" - [open remote]({url})")).unwrap_or_default();

        self.response_builder
            .push(&format!("**{}** ({}) {remote_link}", specification.name, specification.version), HoverCategory::Title);
        self.response_builder.push(&information, HoverCategory::Documentation);
    }
}

/// Mirrors the `/(<<(?<type>(-|~)?))(?<quote>['"`]?)(?<delimiter>\w+)\k<quote>/` regular expression, anchored at the
/// start because heredoc openings always begin with `<<`
fn match_heredoc_opening(opening_content: &str) -> Option<(Option<char>, String)> {
    let rest = opening_content.strip_prefix("<<")?;
    let mut characters = rest.chars().peekable();
    let heredoc_type = characters.next_if(|character| *character == '-' || *character == '~');
    let quote = characters.next_if(|character| matches!(character, '\'' | '"' | '`'));
    let delimiter: String =
        std::iter::from_fn(|| characters.next_if(|character| character.is_ascii_alphanumeric() || *character == '_')).collect();
    if delimiter.is_empty() {
        return None;
    }
    if let Some(quote) = quote
        && characters.next() != Some(quote)
    {
        return None;
    }
    Some((heredoc_type, delimiter))
}

fn remove_leading_spaces_of_each_line(text: &str) -> String {
    text.split_inclusive('\n').map(|line| line.trim_start_matches(' ')).collect()
}

#[derive(Debug, Deserialize)]
struct GemSpecification {
    name: String,
    version: String,
    description: Option<String>,
    summary: Option<String>,
    homepage: Option<String>,
    source_code_uri: Option<String>,
}

const GEM_SPECIFICATION_SCRIPT: &str = r#"
require "json"
name, version = ARGV
begin
  spec = version ? Gem::Specification.find_by_name(name, version) : Gem::Specification.find_by_name(name)
  print JSON.generate(
    name: spec.name,
    version: spec.version.to_s,
    description: spec.description,
    summary: spec.summary,
    homepage: spec.homepage,
    source_code_uri: spec.metadata["source_code_uri"],
  )
rescue Gem::MissingSpecError
  print "null"
end
"#;

/// Gem specifications are only available to RubyGems, so they are read by running Ruby. When the bundle is known, only
/// its locked gems are considered, like `Gem::Specification.find_by_name` inside Ruby LSP's bundle
fn find_gem_specification(global_state: &GlobalState, name: &str) -> Option<GemSpecification> {
    let locked_gems = &global_state.ruby_environment.locked_gems;
    let locked_version = locked_gems.iter().find(|gem| gem.name == name).map(|gem| gem.version.clone());
    if !locked_gems.is_empty() && locked_version.is_none() {
        return None;
    }

    let mut command = Command::new("ruby");
    command.arg("-e").arg(GEM_SPECIFICATION_SCRIPT).arg(name);
    if let Some(version) = &locked_version {
        command.arg(version);
    }
    let workspace_path = global_state.workspace_path();
    if std::path::Path::new(&workspace_path).is_dir() {
        command.current_dir(&workspace_path);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice::<Option<GemSpecification>>(&output.stdout).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_heredoc_openings() {
        assert_eq!(match_heredoc_opening("<<~RUBY"), Some((Some('~'), "RUBY".to_string())));
        assert_eq!(match_heredoc_opening("<<-'EOS'"), Some((Some('-'), "EOS".to_string())));
        assert_eq!(match_heredoc_opening("<<\"A\""), Some((None, "A".to_string())));
        assert_eq!(match_heredoc_opening("<<\"A-B\""), None);
        assert_eq!(match_heredoc_opening("\""), None);
    }

    #[test]
    fn removes_leading_spaces_of_each_line() {
        assert_eq!(remove_leading_spaces_of_each_line("  a\n   b\n"), "a\nb\n");
    }
}
