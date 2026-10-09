use type_checker::Severity;

use crate::type_checking;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Like rustc: the location, the source line and carets under the reported range
    #[default]
    Pretty,
    /// One line per diagnostic, `path:line:column: [severity] message (code)`, for tools that parse the output
    Line,
}

impl OutputFormat {
    pub fn from_name(name: &str) -> Option<OutputFormat> {
        match name {
            "pretty" => Some(OutputFormat::Pretty),
            "line" => Some(OutputFormat::Line),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedDiagnostic {
    pub path: String,
    pub severity: Severity,
    pub code: Option<&'static str>,
    pub message: String,
    pub location: Option<ReportedLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedLocation {
    /// 1-based
    pub line: usize,
    /// 1-based column as printed by the line format
    pub column: usize,
    pub source_line: String,
    /// Character range of the reported range within `source_line`, cut at the end of the line
    pub start_character: usize,
    pub end_character: usize,
}

impl ReportedLocation {
    /// `start` and `end` are byte offsets into `source`; the reported range is shown on the line of `start`
    pub fn from_offsets(source: &[u8], start: usize, end: usize, column: usize) -> ReportedLocation {
        let start = start.min(source.len());
        let line_start = source[..start].iter().rposition(|byte| *byte == b'\n').map_or(0, |index| index + 1);
        let line_end = source[start..].iter().position(|byte| *byte == b'\n').map_or(source.len(), |index| start + index);
        let line_end = if line_end > line_start && source[line_end - 1] == b'\r' { line_end - 1 } else { line_end };
        let line = source[..line_start].iter().filter(|byte| **byte == b'\n').count() + 1;
        let character_count = |bytes: &[u8]| String::from_utf8_lossy(bytes).chars().count();
        let start_character = character_count(&source[line_start..start.min(line_end)]);
        let end_character = character_count(&source[line_start..end.clamp(start, line_end)]);
        ReportedLocation {
            line,
            column,
            source_line: String::from_utf8_lossy(&source[line_start..line_end]).into_owned(),
            start_character,
            end_character,
        }
    }
}

pub struct Renderer {
    pub format: OutputFormat,
    pub colored: bool,
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const BLUE: &str = "\x1b[1;34m";

impl Renderer {
    pub fn render(&self, diagnostic: &ReportedDiagnostic) -> String {
        match self.format {
            OutputFormat::Line => render_line(diagnostic),
            OutputFormat::Pretty => self.render_pretty(diagnostic),
        }
    }

    fn paint(&self, color: &str, text: &str) -> String {
        if self.colored { format!("{color}{text}{RESET}") } else { text.to_string() }
    }

    fn render_pretty(&self, diagnostic: &ReportedDiagnostic) -> String {
        let severity_color = match diagnostic.severity {
            Severity::Error => "\x1b[1;31m",
            Severity::Warning => "\x1b[1;33m",
            Severity::Information => "\x1b[1;36m",
            Severity::Hint => "\x1b[1;32m",
        };
        let mut message_lines = diagnostic.message.lines();
        let title = match diagnostic.code {
            Some(code) => format!("{}[{code}]", type_checking::severity_name(diagnostic.severity)),
            None => type_checking::severity_name(diagnostic.severity).to_string(),
        };
        let mut output = format!(
            "{}{}",
            self.paint(severity_color, &title),
            self.paint(BOLD, &format!(": {}", message_lines.next().unwrap_or_default()))
        );
        let Some(location) = &diagnostic.location else {
            output.push_str(&format!("\n {} {}", self.paint(BLUE, "-->"), diagnostic.path));
            return output;
        };
        let line_number = location.line.to_string();
        let gutter = " ".repeat(line_number.len());
        let bar = self.paint(BLUE, "|");
        output.push_str(&format!(
            "\n{gutter}{} {}:{}:{}",
            self.paint(BLUE, "-->"),
            diagnostic.path,
            location.line,
            location.column
        ));
        output.push_str(&format!("\n{gutter} {bar}"));
        let characters: Vec<char> = location.source_line.chars().collect();
        let shown_line: String = characters.iter().map(|character| expand_tab(*character)).collect();
        output.push_str(&format!("\n{} {bar} {shown_line}", self.paint(BLUE, &line_number)));
        let start = location.start_character.min(characters.len());
        let end = location.end_character.clamp(start, characters.len());
        let padding = display_width(&characters[..start]);
        let caret_count = display_width(&characters[start..end]).max(1);
        output.push_str(&format!(
            "\n{gutter} {bar} {}{}",
            " ".repeat(padding),
            self.paint(severity_color, &"^".repeat(caret_count))
        ));
        let rest: Vec<&str> = message_lines.collect();
        if !rest.is_empty() {
            output.push_str(&format!("\n{gutter} {bar}"));
            for line in rest {
                output.push_str(&format!("\n{gutter} {} {line}", self.paint(BLUE, "=")));
            }
        }
        output
    }
}

pub fn render_line(diagnostic: &ReportedDiagnostic) -> String {
    let location = diagnostic.location.as_ref().map(|location| format!(":{}:{}", location.line, location.column)).unwrap_or_default();
    let code = diagnostic.code.map(|code| format!(" ({code})")).unwrap_or_default();
    format!(
        "{}{location}: [{}] {}{code}",
        diagnostic.path,
        type_checking::severity_name(diagnostic.severity),
        diagnostic.message
    )
}

fn expand_tab(character: char) -> String {
    if character == '\t' { "    ".to_string() } else { character.to_string() }
}

fn display_width(characters: &[char]) -> usize {
    characters.iter().map(|character| character_width(*character)).sum()
}

/// Terminals draw East Asian wide characters in two columns, so the carets have to skip two spaces for each
fn character_width(character: char) -> usize {
    if character == '\t' {
        return 4;
    }
    let code_point = character as u32;
    let is_wide = matches!(code_point,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD);
    if is_wide { 2 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(source: &str, start: usize, end: usize, message: &str) -> ReportedDiagnostic {
        let location = ReportedLocation::from_offsets(source.as_bytes(), start, end, 7);
        ReportedDiagnostic {
            path: "app/models/user.rb".to_string(),
            severity: Severity::Error,
            code: Some("Ruby::NoMethod"),
            message: message.to_string(),
            location: Some(location),
        }
    }

    #[test]
    fn renders_like_rustc() {
        let source = "class User\n  def broken\n    entries.fnd(1)\n  end\nend\n";
        let start = source.find("fnd").unwrap();
        let diagnostic = diagnostic(source, start, start + 3, "Type `::Entries` does not have method `fnd`");
        let renderer = Renderer { format: OutputFormat::Pretty, colored: false };
        assert_eq!(
            renderer.render(&diagnostic),
            [
                "error[Ruby::NoMethod]: Type `::Entries` does not have method `fnd`",
                " --> app/models/user.rb:3:7",
                "  |",
                "3 |     entries.fnd(1)",
                "  |             ^^^",
            ]
            .join("\n")
        );
    }

    #[test]
    fn shows_the_rest_of_a_multiline_message_as_notes_and_cuts_the_carets_at_the_end_of_the_line() {
        let source = "call(1,\n  2)\n";
        let diagnostic = diagnostic(source, 0, source.len(), "Cannot pass a value\n  Integer <: String");
        let renderer = Renderer { format: OutputFormat::Pretty, colored: false };
        assert_eq!(
            renderer.render(&diagnostic),
            [
                "error[Ruby::NoMethod]: Cannot pass a value",
                " --> app/models/user.rb:1:7",
                "  |",
                "1 | call(1,",
                "  | ^^^^^^^",
                "  |",
                "  =   Integer <: String",
            ]
            .join("\n")
        );
    }

    #[test]
    fn aligns_carets_after_wide_characters() {
        let source = "名前 = nil.upcase\n";
        let start = source.find("upcase").unwrap();
        let diagnostic = diagnostic(source, start, start + 6, "message");
        let renderer = Renderer { format: OutputFormat::Pretty, colored: false };
        assert!(renderer.render(&diagnostic).ends_with(&format!("  | {}^^^^^^", " ".repeat(11))));
    }

    #[test]
    fn renders_one_line_per_diagnostic() {
        let source = "x.fnd\n";
        let diagnostic = diagnostic(source, 2, 5, "message");
        assert_eq!(render_line(&diagnostic), "app/models/user.rb:1:7: [error] message (Ruby::NoMethod)");
        let without_location = ReportedDiagnostic { location: None, code: None, ..diagnostic };
        assert_eq!(render_line(&without_location), "app/models/user.rb: [error] message");
    }
}
