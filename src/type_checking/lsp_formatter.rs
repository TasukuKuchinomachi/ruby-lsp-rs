use rbs_definition::ast::{ClassOrModule, Declaration, Type as RbsType, TypeName, TypeParameter, Variance};
use rbs_definition::definition_builder::{DefinitionMember, Method};
use steep::method_name::MethodName;
use steep::services::completion_provider::CompletionItem;
use steep::services::hover_provider::{CallKind, HoverContent, HoverMethodCall};
use steep::type_inference::method_call::MethodDeclaration;
use steep_interface::method_type::MethodType;

#[derive(Default)]
struct Output(String);

impl Output {
    fn puts(&mut self, text: &str) {
        self.0.push_str(text);
        if !text.ends_with('\n') {
            self.0.push('\n');
        }
    }

    fn write(&mut self, text: &str) {
        self.0.push_str(text);
    }
}

pub fn format_hover_content(content: &HoverContent) -> String {
    match content {
        HoverContent::Variable { name, type_, .. } => local_variable(name, &type_.to_string()),
        HoverContent::MethodCall { method_call, .. } => method_call_content(method_call),
        HoverContent::Definition {
            method_name,
            method_type,
            definition,
            ..
        } => definition_content(method_name, method_type, definition),
        HoverContent::Constant {
            full_name,
            declaration,
            comments,
            ..
        } => {
            let mut output = Output::default();
            output.puts(&format!("```rbs\n{}\n```\n", declaration_summary(declaration)));
            if !comments.is_empty() {
                output.puts("----");
                let titled = comments
                    .iter()
                    .map(|comment| (full_name.relative().to_string(), Some(comment.clone())))
                    .collect();
                output.puts(&format_comments(titled));
            }
            output.0
        }
        HoverContent::Type { type_, .. } => format!("```rbs\n{type_}\n```\n"),
        HoverContent::TypeAssertion {
            original_type,
            asserted_type,
            ..
        } => {
            format!("```rbs\n{asserted_type}\n```\n\n↑ Converted from `{original_type}`\n")
        }
        HoverContent::TypeAlias { declaration, .. } | HoverContent::InterfaceType { declaration, .. } => {
            let mut output = Output::default();
            output.puts(&format!("```rbs\n{}\n```\n", declaration_summary(declaration)));
            let (name, comment) = match declaration {
                Declaration::TypeAlias(alias) => (&alias.name, alias.comment.as_deref()),
                Declaration::Interface(interface) => (&interface.name, interface.comment.as_deref()),
                _ => return output.0,
            };
            if let Some(comment) = comment {
                output.puts("");
                output.puts("----");
                output.puts(&format_comment(comment, Some(&name.relative().to_string())));
            }
            output.0
        }
        HoverContent::ClassType { declaration, .. } => {
            let mut output = Output::default();
            output.write(&format!("```rbs\n{}\n```\n", declaration_summary(declaration)));
            let class_comment = match declaration {
                Declaration::Class(class) if !class.declared_in_ruby => {
                    class.comment.as_deref().map(|comment| (&class.name, comment))
                }
                Declaration::Module(module) if !module.declared_in_ruby => {
                    module.comment.as_deref().map(|comment| (&module.name, comment))
                }
                Declaration::ClassAlias(alias) if !alias.declared_in_ruby => {
                    alias.comment.as_deref().map(|comment| (&alias.new_name, comment))
                }
                _ => None,
            };
            if let Some((class_name, comment)) = class_comment {
                output.puts("----");
                output.write(&format_comments(vec![(
                    class_name.relative().to_string(),
                    Some(comment.to_string()),
                )]));
            }
            output.0
        }
    }
}

fn unique_declarations(declarations: &[MethodDeclaration]) -> Vec<&MethodDeclaration> {
    let mut unique: Vec<&MethodDeclaration> = Vec::new();
    for declaration in declarations {
        if !unique.contains(&declaration) {
            unique.push(declaration);
        }
    }
    unique.sort_by_key(|declaration| declaration.name().to_string());
    unique
}

fn method_call_content(call: &HoverMethodCall) -> String {
    let mut output = Output::default();
    let declarations = unique_declarations(&call.method_decls);
    let mut method_types: Vec<String> = declarations
        .iter()
        .map(|declaration| declaration.method_type.to_string())
        .collect();
    let header = match &call.kind {
        CallKind::Typed { actual_method_type } => {
            output.puts(&format!(
                "```rbs\n{}\n```\n\n----\n",
                actual_method_type.type_.return_type
            ));
            None
        }
        CallKind::Special { actual_method_type } => {
            output.puts(&format!(
                "```rbs\n{}\n```\n\n----\n",
                actual_method_type.type_.return_type
            ));
            let mut special_method_type = actual_method_type.clone();
            special_method_type.type_.return_type = call.return_type.clone();
            method_types = vec![special_method_type.to_string()];
            Some("**💡 Custom typing rule applies**\n\n----\n")
        }
        CallKind::Error => Some("**🚨 No compatible method type found**\n\n----\n"),
    };
    let method_names: Vec<String> = declarations
        .iter()
        .map(|declaration| declaration.name().relative().to_string())
        .collect();
    let mut comments: Vec<(String, Option<String>)> = Vec::new();
    for declaration in &declarations {
        let name = declaration.name().relative().to_string();
        let comment = declaration.comment().map(str::to_string);
        match comments.iter_mut().find(|(existing, _)| *existing == name) {
            Some(entry) => entry.1 = comment,
            None => comments.push((name, comment)),
        }
    }
    if let Some(header) = header {
        output.puts(header);
    }
    output.puts(&format_method_item_doc(&method_types, &method_names, comments, ""));
    output.0
}

fn definition_content(method_name: &MethodName, method_type: &MethodType, definition: &Method) -> String {
    let mut output = Output::default();
    let shown_name = match method_name {
        MethodName::Singleton { method_name, .. } => format!("self.{method_name}"),
        MethodName::Instance { method_name, .. } => method_name.clone(),
    };
    let prefix_size = "def ".len() + shown_name.chars().count();
    let method_types: Vec<String> = definition.method_types().map(ToString::to_string).collect();
    let separator = format!("\n{}| ", " ".repeat(prefix_size));
    output.puts(&format!(
        "```rbs\ndef {shown_name}: {}\n```\n\n----\n",
        method_types.join(&separator)
    ));
    if method_types.len() > 1 {
        output.puts("**Internal method type**");
        output.puts(&format!("```rbs\n{method_type}\n```\n\n----\n"));
    }
    let title = method_name.relative().to_string();
    output.puts(&format_comments(
        method_comments(definition)
            .into_iter()
            .map(|comment| (title.clone(), Some(comment)))
            .collect(),
    ));
    output.0
}

fn method_comments(definition: &Method) -> Vec<String> {
    let mut comments: Vec<String> = Vec::new();
    for type_definition in &definition.defs {
        if let Some(comment) = member_comment(&type_definition.member)
            && !comments.iter().any(|existing| existing == comment)
        {
            comments.push(comment.to_string());
        }
    }
    comments
}

pub fn member_comment(member: &DefinitionMember) -> Option<&str> {
    match member {
        DefinitionMember::MethodDefinition(definition) if !definition.declared_in_ruby => definition.comment.as_deref(),
        DefinitionMember::Attribute(attribute) if !attribute.declared_in_ruby => attribute.comment.as_deref(),
        _ => None,
    }
}

pub fn format_completion_documentation(item: &CompletionItem, type_name_declaration: Option<&Declaration>) -> String {
    match item {
        CompletionItem::LocalVariable { identifier, type_, .. } => local_variable(identifier, &type_.to_string()),
        CompletionItem::Constant {
            declaration,
            comments,
            full_name,
            ..
        } => {
            let mut output = Output::default();
            let summary = declaration.as_ref().map(declaration_summary).unwrap_or_default();
            output.puts(&format!("```rbs\n{summary}\n```\n"));
            if !comments.is_empty() {
                output.puts("----");
                let titled = comments
                    .iter()
                    .map(|comment| (full_name.relative().to_string(), Some(comment.clone())))
                    .collect();
                output.puts(&format_comments(titled));
            }
            output.0
        }
        CompletionItem::InstanceVariable { identifier, type_, .. } => {
            format!("**Instance variable** `{identifier}: {type_}`\n")
        }
        CompletionItem::SimpleMethodName {
            method_types,
            method_member,
            method_name,
            ..
        } => {
            let method_types: Vec<String> = method_types.iter().map(ToString::to_string).collect();
            let comments = vec![(
                method_name.relative().to_string(),
                member_comment(method_member).map(str::to_string),
            )];
            format_method_item_doc(&method_types, &[], comments, "")
        }
        CompletionItem::ComplexMethodName {
            method_types,
            method_decls,
            ..
        } => {
            let method_types: Vec<String> = method_types.iter().map(ToString::to_string).collect();
            let mut method_names: Vec<String> = Vec::new();
            let mut comments: Vec<(String, Option<String>)> = Vec::new();
            for declaration in method_decls {
                let name = declaration.name().relative().to_string();
                if !method_names.contains(&name) {
                    method_names.push(name.clone());
                }
                let comment = member_comment(&declaration.definition.member).map(str::to_string);
                match comments.iter_mut().find(|(existing, _)| *existing == name) {
                    Some(entry) => entry.1 = comment,
                    None => comments.push((name, comment)),
                }
            }
            format_method_item_doc(&method_types, &method_names, comments, "")
        }
        CompletionItem::GeneratedMethodName { method_types, .. } => {
            let method_types: Vec<String> = method_types.iter().map(ToString::to_string).collect();
            format_method_item_doc(&method_types, &[], Vec::new(), "🤖 Generated method for receiver type")
        }
        CompletionItem::TypeName { absolute_type_name, .. } => {
            let mut output = Output::default();
            let summary = type_name_declaration.map(declaration_summary).unwrap_or_default();
            output.puts(&format!("```rbs\n{summary}\n```\n"));
            let comments: Vec<String> = type_name_declaration
                .and_then(declaration_comment)
                .map(str::to_string)
                .into_iter()
                .collect();
            if !comments.is_empty() {
                output.puts("----");
                let title = absolute_type_name.relative().to_string();
                output.puts(&format_comments(
                    comments
                        .into_iter()
                        .map(|comment| (title.clone(), Some(comment)))
                        .collect(),
                ));
            }
            output.0
        }
        CompletionItem::KeywordArgument { identifier, .. } => format!("**Keyword argument**: `{identifier}`\n"),
        CompletionItem::Text { .. } => String::new(),
    }
}

pub fn format_rbs_completion_documentation(
    type_name: &TypeName,
    declaration: &Declaration,
    comments: &[String],
) -> String {
    let mut output = Output::default();
    output.puts(&format!("```rbs\n{}\n```\n", declaration_summary(declaration)));
    if !comments.is_empty() {
        output.puts("");
        output.puts("----");
        let title = type_name.relative().to_string();
        output.puts(&format_comments(
            comments
                .iter()
                .map(|comment| (title.clone(), Some(comment.clone())))
                .collect(),
        ));
    }
    output.0
}

pub fn declaration_comment(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Class(class) if !class.declared_in_ruby => class.comment.as_deref(),
        Declaration::Module(module) if !module.declared_in_ruby => module.comment.as_deref(),
        Declaration::Interface(interface) => interface.comment.as_deref(),
        Declaration::TypeAlias(alias) => alias.comment.as_deref(),
        Declaration::Constant(constant) if !constant.declared_in_ruby => constant.comment.as_deref(),
        Declaration::Global(global) => global.comment.as_deref(),
        Declaration::ClassAlias(alias) if !alias.declared_in_ruby => alias.comment.as_deref(),
        _ => None,
    }
}

fn format_comments(comments: Vec<(String, Option<String>)>) -> String {
    let mut output = Output::default();
    let mut with_documentation: Vec<(String, String)> = Vec::new();
    let mut without_documentation_count = 0;
    for (title, comment) in comments {
        match comment {
            Some(comment) => with_documentation.push((title, comment)),
            None => without_documentation_count += 1,
        }
    }
    if !with_documentation.is_empty() {
        for (title, comment) in &with_documentation {
            output.puts(&format_comment(comment, Some(title)));
            output.puts("");
        }
        if without_documentation_count > 0 {
            output.puts("");
            output.puts("----");
            if without_documentation_count == 1 {
                output.puts("🔍 One more definition without docs");
            } else {
                output.puts(&format!(
                    "🔍 {without_documentation_count} more definitions without docs"
                ));
            }
        }
    }
    output.0
}

fn format_comment(comment: &str, header: Option<&str>) -> String {
    let mut output = Output::default();
    if let Some(header) = header {
        output.puts(&format!("### 📚 {}", header.replace('_', "\\_")));
        output.puts("");
    }
    let without_html_comments = remove_html_comment_lines(comment.trim_end());
    output.puts(without_leading_blank_lines(&without_html_comments));
    output.0
}

fn remove_html_comment_lines(text: &str) -> String {
    let mut result = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let line_end = rest.find('\n').map_or(rest.len(), |index| index + 1);
        let indentation = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let after_indentation = &rest[indentation..];
        if after_indentation.starts_with("<!--")
            && let Some(close) = after_indentation.find("-->")
            && after_indentation[close + 3..].starts_with('\n')
        {
            rest = &after_indentation[close + 4..];
            continue;
        }
        result.push_str(&rest[..line_end]);
        rest = &rest[line_end..];
    }
    result
}

fn without_leading_blank_lines(text: &str) -> &str {
    let mut rest = text;
    loop {
        let Some(newline) = rest.find('\n') else { return rest };
        if rest[..newline].trim_matches([' ', '\t']).is_empty() {
            rest = &rest[newline + 1..];
        } else {
            return rest;
        }
    }
}

pub fn remove_html_comments(text: &str) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        let Some(close) = rest[start..].find("-->") else { break };
        result.push_str(&rest[..start]);
        rest = &rest[start + close + 3..];
    }
    result.push_str(rest);
    result
}

fn local_variable(name: &str, type_text: &str) -> String {
    format!("**Local variable** `{name}: {type_text}`\n")
}

fn name_and_parameters(name: &str, parameters: &[TypeParameter]) -> String {
    if parameters.is_empty() {
        return name.to_string();
    }
    let parameters: Vec<String> = parameters
        .iter()
        .map(|parameter| {
            let mut text = String::new();
            if parameter.unchecked {
                text.push_str("unchecked ");
            }
            match parameter.variance {
                Variance::Invariant => {}
                Variance::Covariant => text.push_str("out "),
                Variance::Contravariant => text.push_str("in "),
            }
            text.push_str(&parameter.name);
            if let Some(upper_bound) = &parameter.upper_bound {
                text.push_str(&format!(" < {upper_bound}"));
            }
            text
        })
        .collect();
    format!("{name}[{}]", parameters.join(", "))
}

fn name_and_arguments(name: &TypeName, arguments: &[RbsType]) -> String {
    if arguments.is_empty() {
        return name.to_string();
    }
    let arguments: Vec<String> = arguments.iter().map(ToString::to_string).collect();
    format!("{name}[{}]", arguments.join(", "))
}

pub fn declaration_summary(declaration: &Declaration) -> String {
    match declaration {
        Declaration::Class(class) if class.declared_in_ruby => format!("class {}", class.name.relative()),
        Declaration::Class(class) => {
            let super_class = class
                .super_class
                .as_ref()
                .map(|super_class| format!(" < {}", name_and_arguments(&super_class.name, &super_class.args)))
                .unwrap_or_default();
            format!(
                "class {}{super_class}",
                name_and_parameters(&class.name.relative().to_string(), &class.type_parameters)
            )
        }
        Declaration::Module(module) if module.declared_in_ruby => format!("module {}", module.name.relative()),
        Declaration::Module(module) => {
            let self_types = if module.self_types.is_empty() {
                String::new()
            } else {
                let self_types: Vec<String> = module
                    .self_types
                    .iter()
                    .map(|self_type| name_and_arguments(&self_type.name, &self_type.args))
                    .collect();
                format!(" : {}", self_types.join(", "))
            };
            format!(
                "module {}{self_types}",
                name_and_parameters(&module.name.relative().to_string(), &module.type_parameters)
            )
        }
        Declaration::TypeAlias(alias) => format!(
            "type {} = {}",
            name_and_parameters(&alias.name.relative().to_string(), &alias.type_parameters),
            alias.type_
        ),
        Declaration::Interface(interface) => {
            format!(
                "interface {}",
                name_and_parameters(&interface.name.relative().to_string(), &interface.type_parameters)
            )
        }
        Declaration::ClassAlias(alias) => {
            let keyword = match alias.kind {
                ClassOrModule::Class => "class",
                ClassOrModule::Module => "module",
            };
            if alias.declared_in_ruby {
                format!("{keyword} {} = {}", alias.new_name, alias.old_name)
            } else {
                format!("{keyword} {} = {}", alias.new_name.relative(), alias.old_name)
            }
        }
        Declaration::Global(global) => format!("{}: {}", global.name, global.type_),
        Declaration::Constant(constant) => format!("{}: {}", constant.name.relative(), constant.type_),
    }
}

fn format_method_item_doc(
    method_types: &[String],
    method_names: &[String],
    comments: Vec<(String, Option<String>)>,
    footer: &str,
) -> String {
    let mut output = Output::default();
    output.puts("**Method type**:");
    output.puts("```rbs");
    if method_types.len() == 1 {
        output.puts(&method_types[0]);
    } else {
        output.puts(&format!("  {}", method_types.join("\n| ")));
    }
    output.puts("```");
    if method_names.len() > 1 {
        let names: Vec<String> = method_names.iter().map(|name| format!("`{name}`")).collect();
        output.puts(&format!("**Possible methods**: {}", names.join(", ")));
        output.puts("");
    }
    if comments.iter().any(|(_, comment)| comment.is_some()) {
        output.puts("----");
        output.puts(&format_comments(comments));
    }
    if !footer.is_empty() {
        output.puts(footer.trim_end());
    }
    output.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_comments_with_their_headers() {
        let formatted = format_comments(vec![
            ("Foo#bar_baz".to_string(), Some("Does things.\n".to_string())),
            ("Foo#other".to_string(), None),
        ]);
        assert_eq!(
            formatted,
            "### 📚 Foo#bar\\_baz\n\nDoes things.\n\n\n----\n🔍 One more definition without docs\n"
        );
    }

    #[test]
    fn removes_html_comment_lines_and_leading_blank_lines() {
        assert_eq!(format_comment("\n<!-- hidden -->\nShown\n", None), "Shown\n");
        assert_eq!(remove_html_comments("a<!-- b -->c"), "ac");
    }

    #[test]
    fn formats_a_method_item_with_overloads() {
        let formatted = format_method_item_doc(
            &["() -> ::String".to_string(), "(::Integer) -> ::String".to_string()],
            &["String#a".to_string(), "String#b".to_string()],
            vec![("String#a".to_string(), None)],
            "",
        );
        assert_eq!(
            formatted,
            "**Method type**:\n```rbs\n  () -> ::String\n| (::Integer) -> ::String\n```\n**Possible methods**: `String#a`, `String#b`\n\n"
        );
    }
}
