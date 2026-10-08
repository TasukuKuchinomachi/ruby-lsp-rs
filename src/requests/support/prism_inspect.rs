use ruby_prism::{ConstantId, ConstantList, Integer, Location, Node};

use super::prism_inspect_fields::node_fields;
use crate::position::LineIndex;

pub enum Field<'pr> {
    Child(&'static str, Option<Node<'pr>>),
    Children(&'static str, Vec<Node<'pr>>),
    Value(&'static str, String),
}

pub struct Inspector<'a> {
    line_index: &'a LineIndex<'a>,
}

impl<'a> Inspector<'a> {
    pub fn new(line_index: &'a LineIndex<'a>) -> Inspector<'a> {
        Inspector { line_index }
    }

    pub fn inspect(&self, node: &Node<'_>) -> String {
        let mut output = String::new();
        self.write_node(&mut output, node, "", "");
        output
    }

    fn write_node(&self, output: &mut String, node: &Node<'_>, first_line_prefix: &str, indent: &str) {
        let (name, fields) = node_fields(self, node);
        let location = node.location();
        output.push_str(first_line_prefix);
        output.push_str(&format!("@ {name} (location: {})\n", self.location_bounds(location.start_offset(), location.end_offset())));

        let field_count = fields.len();
        for (field_index, field) in fields.into_iter().enumerate() {
            let last = field_index + 1 == field_count;
            let (branch, child_indent) = if last { ("└── ", "    ") } else { ("├── ", "│   ") };
            let nested_indent = format!("{indent}{child_indent}");
            output.push_str(indent);
            output.push_str(branch);
            match field {
                Field::Value(field_name, value) => output.push_str(&format!("{field_name}: {value}\n")),
                Field::Child(field_name, None) => output.push_str(&format!("{field_name}: ∅\n")),
                Field::Child(field_name, Some(child)) => {
                    output.push_str(&format!("{field_name}:\n"));
                    self.write_node(output, &child, &nested_indent, &nested_indent);
                }
                Field::Children(field_name, children) => {
                    output.push_str(&format!("{field_name}: (length: {})\n", children.len()));
                    let child_count = children.len();
                    for (child_index, child) in children.iter().enumerate() {
                        let (child_branch, grandchild_indent) =
                            if child_index + 1 == child_count { ("└── ", "    ") } else { ("├── ", "│   ") };
                        self.write_node(
                            output,
                            child,
                            &format!("{nested_indent}{child_branch}"),
                            &format!("{nested_indent}{grandchild_indent}"),
                        );
                    }
                }
            }
        }
    }

    fn location_bounds(&self, start_offset: usize, end_offset: usize) -> String {
        let (start_line, start_column) = self.line_and_column(start_offset);
        let (end_line, end_column) = self.line_and_column(end_offset);
        format!("({start_line},{start_column})-({end_line},{end_column})")
    }

    fn line_and_column(&self, offset: usize) -> (usize, usize) {
        let line = self.line_index.line_of_offset(offset);
        let line_start = self.line_index.line_start(line).unwrap_or(0);
        (line + 1, offset.saturating_sub(line_start))
    }

    pub fn location(&self, location: Location<'_>) -> String {
        format!(
            "{} = {}",
            self.location_bounds(location.start_offset(), location.end_offset()),
            inspect_string(location.as_slice(), false)
        )
    }

    pub fn optional_location(&self, location: Option<Location<'_>>) -> String {
        match location {
            Some(location) => self.location(location),
            None => "∅".to_string(),
        }
    }
}

pub fn inspect_flags(flags: &[(bool, &str)]) -> String {
    let names: Vec<&str> = flags.iter().filter(|(enabled, _)| *enabled).map(|(_, name)| *name).collect();
    if names.is_empty() { "∅".to_string() } else { names.join(", ") }
}

pub fn inspect_constant(constant: &ConstantId<'_>) -> String {
    inspect_symbol(constant.as_slice())
}

pub fn inspect_optional_constant(constant: Option<ConstantId<'_>>) -> String {
    match constant {
        Some(constant) => inspect_constant(&constant),
        None => "∅".to_string(),
    }
}

pub fn inspect_constant_list(constants: &ConstantList<'_>) -> String {
    let symbols: Vec<String> = constants.iter().map(|constant| inspect_constant(&constant)).collect();
    format!("[{}]", symbols.join(", "))
}

pub fn inspect_integer(integer: &Integer<'_>) -> String {
    let (negative, digits) = integer.to_u32_digits();
    let mut remaining: Vec<u32> = digits.to_vec();
    while remaining.last() == Some(&0) && remaining.len() > 1 {
        remaining.pop();
    }

    let mut decimal_digits = Vec::new();
    loop {
        let mut remainder: u64 = 0;
        for digit in remaining.iter_mut().rev() {
            let value = (remainder << 32) | u64::from(*digit);
            *digit = (value / 10) as u32;
            remainder = value % 10;
        }
        decimal_digits.push(char::from(b'0' + remainder as u8));
        while remaining.last() == Some(&0) && remaining.len() > 1 {
            remaining.pop();
        }
        if remaining == [0] {
            break;
        }
    }

    let mut result = String::new();
    if negative && decimal_digits != ['0'] {
        result.push('-');
    }
    result.extend(decimal_digits.iter().rev());
    result
}

pub fn inspect_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() };
    }
    if value == 0.0 {
        return if value.is_sign_negative() { "-0.0".to_string() } else { "0.0".to_string() };
    }

    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("scientific notation has an exponent");
    let exponent: i32 = exponent.parse().expect("exponent is an integer");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let decimal_point = exponent + 1;
    let sign = if value < 0.0 { "-" } else { "" };

    if 0 < decimal_point && decimal_point < 16 {
        let decimal_point = decimal_point as usize;
        if digits.len() <= decimal_point {
            format!("{sign}{digits}{}.0", "0".repeat(decimal_point - digits.len()))
        } else {
            format!("{sign}{}.{}", &digits[..decimal_point], &digits[decimal_point..])
        }
    } else if -4 < decimal_point && decimal_point <= 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-decimal_point) as usize))
    } else {
        let fraction = if digits.len() > 1 { &digits[1..] } else { "0" };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        format!("{sign}{}.{fraction}e{exponent_sign}{:02}", &digits[..1], exponent.abs())
    }
}

pub fn inspect_string(bytes: &[u8], binary: bool) -> String {
    let mut result = String::from("\"");
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte < 0x80 {
            let next = bytes.get(index + 1).copied();
            match byte {
                b'"' => result.push_str("\\\""),
                b'\\' => result.push_str("\\\\"),
                b'#' if matches!(next, Some(b'{') | Some(b'$') | Some(b'@')) => result.push_str("\\#"),
                b'\n' => result.push_str("\\n"),
                b'\r' => result.push_str("\\r"),
                b'\t' => result.push_str("\\t"),
                0x0c => result.push_str("\\f"),
                0x0b => result.push_str("\\v"),
                0x08 => result.push_str("\\b"),
                0x07 => result.push_str("\\a"),
                0x1b => result.push_str("\\e"),
                0x20..=0x7e => result.push(char::from(byte)),
                _ if binary => result.push_str(&format!("\\x{byte:02X}")),
                _ => result.push_str(&format!("\\u{:04X}", byte)),
            }
            index += 1;
            continue;
        }

        if binary {
            result.push_str(&format!("\\x{byte:02X}"));
            index += 1;
            continue;
        }

        let character_length = utf8_character_length(&bytes[index..]);
        match character_length.and_then(|length| std::str::from_utf8(&bytes[index..index + length]).ok()) {
            Some(text) => {
                let character = text.chars().next().expect("decoded text has a character");
                if character.is_control() || character == '\u{2028}' || character == '\u{2029}' {
                    let code_point = character as u32;
                    if code_point < 0x10000 {
                        result.push_str(&format!("\\u{code_point:04X}"));
                    } else {
                        result.push_str(&format!("\\u{{{code_point:X}}}"));
                    }
                } else {
                    result.push(character);
                }
                index += text.len();
            }
            None => {
                result.push_str(&format!("\\x{byte:02X}"));
                index += 1;
            }
        }
    }
    result.push('"');
    result
}

fn utf8_character_length(bytes: &[u8]) -> Option<usize> {
    let length = match bytes.first()? {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    (bytes.len() >= length).then_some(length)
}

const OPERATOR_SYMBOLS: &[&str] = &[
    "+", "-", "*", "/", "%", "**", "==", "===", "!=", "=~", "!~", "!", "<", "<=", ">", ">=", "<=>", "<<", ">>", "&", "|",
    "^", "~", "+@", "-@", "[]", "[]=", "`",
];

const SPECIAL_GLOBAL_CHARACTERS: &[u8] = b"~*$?!@/\\;,.=:<>\"&`'+0_";

pub fn inspect_symbol(bytes: &[u8]) -> String {
    let simple = std::str::from_utf8(bytes).is_ok_and(is_simple_symbol);
    if simple {
        format!(":{}", String::from_utf8_lossy(bytes))
    } else {
        format!(":{}", inspect_string(bytes, false))
    }
}

fn is_identifier_start(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_' || !character.is_ascii()
}

fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || !character.is_ascii()
}

fn is_identifier(text: &str) -> bool {
    let mut characters = text.chars();
    characters.next().is_some_and(is_identifier_start) && characters.all(is_identifier_character)
}

fn is_simple_symbol(name: &str) -> bool {
    if OPERATOR_SYMBOLS.contains(&name) {
        return true;
    }
    if let Some(rest) = name.strip_prefix("@@") {
        return is_identifier(rest);
    }
    if let Some(rest) = name.strip_prefix('@') {
        return is_identifier(rest);
    }
    if let Some(rest) = name.strip_prefix('$') {
        if let Some(option) = rest.strip_prefix('-') {
            let mut characters = option.chars();
            return characters.next().is_some_and(is_identifier_character) && characters.next().is_none();
        }
        if rest.len() == 1 && SPECIAL_GLOBAL_CHARACTERS.contains(&rest.as_bytes()[0]) {
            return true;
        }
        if !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()) {
            return true;
        }
        return is_identifier(rest);
    }

    let base = name.strip_suffix(['?', '!', '=']).unwrap_or(name);
    is_identifier(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspects_floats_like_ruby() {
        let cases = [
            (1.0, "1.0"),
            (1e15, "1.0e+15"),
            (1e14, "100000000000000.0"),
            (0.0001, "0.0001"),
            (0.00001, "1.0e-05"),
            (1.5e-7, "1.5e-07"),
            (123.456, "123.456"),
            (-0.0, "-0.0"),
            (2.5e300, "2.5e+300"),
            (1234567890123456.0, "1.234567890123456e+15"),
            (f64::INFINITY, "Infinity"),
        ];
        for (value, expected) in cases {
            assert_eq!(inspect_float(value), expected);
        }
    }

    #[test]
    fn inspects_strings_like_ruby() {
        assert_eq!(
            inspect_string("\x01\x7f\u{85}\u{2028}\u{a0}é#{#$#@#x\x1b\0".as_bytes(), false),
            "\"\\u0001\\u007F\\u0085\\u2028\u{a0}é\\#{\\#$\\#@#x\\e\\u0000\""
        );
        assert_eq!(inspect_string(b"a\xffb", false), r#""a\xFFb""#);
        assert_eq!(inspect_string(b"\x01\xff", true), r#""\x01\xFF""#);
    }

    #[test]
    fn inspects_symbols_like_ruby() {
        let cases = [
            ("foo", ":foo"),
            ("foo bar", ":\"foo bar\""),
            ("[]=", ":[]="),
            ("foo=", ":foo="),
            ("foo?", ":foo?"),
            ("@a", ":@a"),
            ("@@a", ":@@a"),
            ("$1", ":$1"),
            ("$-w", ":$-w"),
            ("1a", ":\"1a\""),
            ("foo?=", ":\"foo?=\""),
            ("=", ":\"=\""),
            ("", ":\"\""),
            ("@", ":\"@\""),
            ("&&", ":\"&&\""),
        ];
        for (name, expected) in cases {
            assert_eq!(inspect_symbol(name.as_bytes()), expected);
        }
    }
}
