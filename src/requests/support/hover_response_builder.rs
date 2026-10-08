use super::common::CategorizedMarkdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverCategory {
    Title,
    Links,
    Documentation,
}

#[derive(Debug, Default)]
pub struct HoverResponseBuilder {
    title: String,
    links: String,
    documentation: String,
}

impl HoverResponseBuilder {
    pub fn push(&mut self, content: &str, category: HoverCategory) {
        let hover_content = match category {
            HoverCategory::Title => &mut self.title,
            HoverCategory::Links => &mut self.links,
            HoverCategory::Documentation => &mut self.documentation,
        };
        hover_content.push_str(content);
        hover_content.push('\n');
    }

    pub fn push_categorized(&mut self, markdown: &CategorizedMarkdown) {
        self.push(&markdown.title, HoverCategory::Title);
        self.push(&markdown.links, HoverCategory::Links);
        self.push(&markdown.documentation, HoverCategory::Documentation);
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.links.is_empty() && self.documentation.is_empty()
    }

    pub fn response(&self) -> String {
        let result = format!("{}\n{}\n{}", self.title, self.links, self.documentation);
        ruby_strip(&result).to_string()
    }
}

/// Ruby's `String#strip` only removes ASCII whitespace and null characters, unlike `str::trim`
pub fn ruby_strip(text: &str) -> &str {
    text.trim_matches(|character: char| matches!(character, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | '\0'))
}
