use std::rc::Rc;

use crate::ruby::node::{Buffer, Child, Location, MapKind, Node, SourceRange};
use crate::ruby::translation::SyntaxError;

#[derive(Clone, Debug)]
pub(super) struct Token {
    pub value: String,
    pub range: SourceRange,
}

impl Token {
    pub fn new(value: impl Into<String>, range: SourceRange) -> Token {
        Token {
            value: value.into(),
            range,
        }
    }
}

pub(super) enum BlockArguments {
    Arguments(Rc<Node>),
    Numbered(u8),
    It,
}

pub(super) struct Builder<'buffer> {
    buffer: &'buffer Buffer,
    pub error: Option<SyntaxError>,
}

fn location(token: Option<&Token>) -> Option<SourceRange> {
    token.map(|token| token.range)
}

fn expression(node: &Node) -> Option<SourceRange> {
    node.location.expression
}

fn optional_expression(node: Option<&Rc<Node>>) -> Option<SourceRange> {
    node.and_then(|node| expression(node))
}

fn join(left: Option<SourceRange>, right: Option<SourceRange>) -> Option<SourceRange> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.join(right)),
        (left, right) => left.or(right),
    }
}

fn join_expressions(left: &Node, right: &Node) -> Option<SourceRange> {
    join(expression(left), expression(right))
}

fn node(node: Rc<Node>) -> Child {
    Child::Node(node)
}

fn optional(node: Option<Rc<Node>>) -> Child {
    node.map_or(Child::Nil, Child::Node)
}

fn symbol(value: impl Into<String>) -> Child {
    Child::Symbol(value.into())
}

fn nodes(nodes: Vec<Rc<Node>>) -> Vec<Child> {
    nodes.into_iter().map(Child::Node).collect()
}

fn build(type_: &'static str, children: Vec<Child>, location: Location) -> Rc<Node> {
    Node::new(type_, children, location)
}

fn build_leaf(type_: &'static str, location: Location) -> Rc<Node> {
    Node::new(type_, Vec::new(), location)
}

fn token_map(token: &Token) -> Location {
    Location::new(MapKind::Map, Some(token.range))
}

fn expression_map(range: Option<SourceRange>) -> Location {
    Location::new(MapKind::Map, range)
}

fn prefix_string_map(range: SourceRange) -> Location {
    let mut map = Location::new(MapKind::Collection, Some(range));
    map.begin = Some(range.with_end_pos(range.begin_pos + 1));
    map
}

fn unquoted_map(range: SourceRange) -> Location {
    Location::new(MapKind::Collection, Some(range))
}

fn collection_map(begin: Option<&Token>, parts: &[Rc<Node>], end: Option<&Token>) -> Location {
    let expression = if begin.is_none() || end.is_none() {
        if let (Some(first), Some(last)) = (parts.first(), parts.last()) {
            join_expressions(first, last)
        } else if begin.is_some() {
            location(begin)
        } else {
            location(end)
        }
    } else {
        join(location(begin), location(end))
    };
    let mut map = Location::new(MapKind::Collection, expression);
    map.begin = location(begin);
    map.end = location(end);
    map
}

fn string_map(begin: Option<&Token>, parts: &[Rc<Node>], end: Option<&Token>) -> Location {
    match begin {
        Some(begin) if begin.value.starts_with("<<") => {
            let body = if let (Some(first), Some(last)) = (parts.first(), parts.last()) {
                join_expressions(first, last)
            } else {
                location(end).map(|end| end.with_end_pos(end.begin_pos))
            };
            let mut map = Location::new(MapKind::Heredoc, Some(begin.range));
            map.heredoc_body = body;
            map.heredoc_end = location(end);
            map
        }
        _ => collection_map(begin, parts, end),
    }
}

fn regexp_map(begin: Option<&Token>, end: Option<&Token>, options: &Node) -> Location {
    let mut map = Location::new(
        MapKind::Collection,
        join(location(begin), expression(options)),
    );
    map.begin = location(begin);
    map.end = location(end);
    map
}

fn constant_map(scope: Option<&Rc<Node>>, double_colon: Option<&Token>, name: &Token) -> Location {
    let expression = match scope {
        None => Some(name.range),
        Some(scope) => join(expression(scope), Some(name.range)),
    };
    let mut map = Location::new(MapKind::Constant, expression);
    map.double_colon = location(double_colon);
    map.name = Some(name.range);
    map
}

fn variable_map(name: &Token) -> Location {
    let mut map = Location::new(MapKind::Variable, Some(name.range));
    map.name = Some(name.range);
    map
}

fn binary_operator_map(left: &Node, operator: Option<&Token>, right: &Node) -> Location {
    let mut map = Location::new(MapKind::Operator, join_expressions(left, right));
    map.operator = location(operator);
    map
}

fn unary_operator_map(operator: &Token, argument: Option<&Rc<Node>>) -> Location {
    let expression = match argument {
        None => Some(operator.range),
        Some(argument) => join(Some(operator.range), expression(argument)),
    };
    let mut map = Location::new(MapKind::Operator, expression);
    map.operator = Some(operator.range);
    map
}

fn range_map(start: Option<&Rc<Node>>, operator: &Token, end: Option<&Rc<Node>>) -> Location {
    let expression = match (start, end) {
        (Some(start), Some(end)) => join_expressions(start, end),
        (Some(start), None) => join(expression(start), Some(operator.range)),
        (None, Some(end)) => join(Some(operator.range), expression(end)),
        (None, None) => None,
    };
    let mut map = Location::new(MapKind::Operator, expression);
    map.operator = Some(operator.range);
    map
}

fn argument_prefix_map(operator: &Token, name: Option<&Token>) -> Location {
    let expression = match name {
        None => operator.range,
        Some(name) => operator.range.join(name.range),
    };
    let mut map = Location::new(MapKind::Variable, Some(expression));
    map.name = location(name);
    map
}

fn keyword_argument_map(name: &Token, value: Option<&Rc<Node>>) -> Location {
    let name_range = name.range.with_end_pos(name.range.end_pos - 1);
    let expression = match value {
        Some(value) => join(Some(name.range), expression(value)),
        None => Some(name.range),
    };
    let mut map = Location::new(MapKind::Variable, expression);
    map.name = Some(name_range);
    map
}

fn module_definition_map(
    keyword: &Token,
    name: Option<&Rc<Node>>,
    operator: Option<&Token>,
    end: &Token,
) -> Location {
    let mut map = Location::new(MapKind::Definition, Some(keyword.range.join(end.range)));
    map.keyword = Some(keyword.range);
    map.operator = location(operator);
    map.name = optional_expression(name);
    map.end = Some(end.range);
    map
}

fn definition_map(
    keyword: &Token,
    operator: Option<&Token>,
    name: &Token,
    end: Option<&Token>,
) -> Location {
    let mut map = Location::new(
        MapKind::MethodDefinition,
        join(Some(keyword.range), location(end)),
    );
    map.keyword = Some(keyword.range);
    map.operator = location(operator);
    map.name = Some(name.range);
    map.end = location(end);
    map
}

fn endless_definition_map(
    keyword: &Token,
    operator: Option<&Token>,
    name: &Token,
    assignment: Option<&Token>,
    body: Option<&Rc<Node>>,
) -> Location {
    let mut map = Location::new(
        MapKind::MethodDefinition,
        join(Some(keyword.range), optional_expression(body)),
    );
    map.keyword = Some(keyword.range);
    map.operator = location(operator);
    map.name = Some(name.range);
    map.assignment = location(assignment);
    map
}

fn send_map(
    receiver: Option<&Rc<Node>>,
    dot: Option<&Token>,
    selector: Option<&Token>,
    begin: Option<&Token>,
    arguments: &[Rc<Node>],
    end: Option<&Token>,
) -> Location {
    let begin_range = match receiver {
        Some(receiver) => expression(receiver),
        None => location(selector),
    };
    let end_range = if end.is_some() {
        location(end)
    } else if let Some(last) = arguments.last() {
        expression(last)
    } else {
        location(selector)
    };
    let mut map = Location::new(MapKind::Send, join(begin_range, end_range));
    map.dot = location(dot);
    map.selector = location(selector);
    map.begin = location(begin);
    map.end = location(end);
    map
}

fn send_binary_operator_map(left: &Node, selector: &Token, right: Option<&Rc<Node>>) -> Location {
    let mut map = Location::new(
        MapKind::Send,
        join(expression(left), optional_expression(right)),
    );
    map.selector = Some(selector.range);
    map
}

fn send_unary_operator_map(selector: &Token, argument: Option<&Rc<Node>>) -> Location {
    let expression = match argument {
        None => Some(selector.range),
        Some(argument) => join(Some(selector.range), expression(argument)),
    };
    let mut map = Location::new(MapKind::Send, expression);
    map.selector = Some(selector.range);
    map
}

fn send_index_map(
    receiver: Option<&Rc<Node>>,
    left_bracket: Option<&Token>,
    right_bracket: Option<&Token>,
) -> Location {
    let mut map = Location::new(
        MapKind::Send,
        join(optional_expression(receiver), location(right_bracket)),
    );
    map.selector = join(location(left_bracket), location(right_bracket));
    map
}

fn block_map(
    receiver: Option<SourceRange>,
    begin: Option<&Token>,
    end: Option<&Token>,
) -> Location {
    let mut map = Location::new(MapKind::Collection, join(receiver, location(end)));
    map.begin = location(begin);
    map.end = location(end);
    map
}

fn keyword_map(
    keyword: &Token,
    begin: Option<&Token>,
    arguments: &[Child],
    end: Option<&Token>,
) -> Location {
    let any = arguments.iter().any(|argument| !argument.is_nil());
    let end_range = if end.is_some() {
        location(end)
    } else if any && !arguments.last().is_some_and(Child::is_nil) {
        arguments
            .last()
            .and_then(Child::node)
            .and_then(|node| expression(node))
    } else if any && arguments.len() > 1 {
        arguments[arguments.len() - 2]
            .node()
            .and_then(|node| expression(node))
    } else {
        Some(keyword.range)
    };
    let mut map = Location::new(MapKind::Keyword, join(Some(keyword.range), end_range));
    map.keyword = Some(keyword.range);
    map.begin = location(begin);
    map.end = location(end);
    map
}

fn keyword_modifier_map(before: Option<&Rc<Node>>, keyword: &Token, after: &Node) -> Location {
    let mut map = Location::new(
        MapKind::Keyword,
        join(optional_expression(before), expression(after)),
    );
    map.keyword = Some(keyword.range);
    map
}

#[allow(clippy::too_many_arguments)]
fn condition_map(
    keyword: &Token,
    condition: Option<&Rc<Node>>,
    begin: Option<&Token>,
    body: Option<&Rc<Node>>,
    else_token: Option<&Token>,
    else_body: Option<&Rc<Node>>,
    end: Option<&Token>,
) -> Location {
    let end_range = if end.is_some() {
        location(end)
    } else if let Some(else_range) = optional_expression(else_body) {
        Some(else_range)
    } else if else_token.is_some() {
        location(else_token)
    } else if let Some(body_range) = optional_expression(body) {
        Some(body_range)
    } else if begin.is_some() {
        location(begin)
    } else {
        optional_expression(condition)
    };
    let mut map = Location::new(MapKind::Condition, join(Some(keyword.range), end_range));
    map.keyword = Some(keyword.range);
    map.begin = location(begin);
    map.else_ = location(else_token);
    map.end = location(end);
    map
}

fn rescue_body_map(
    keyword: &Token,
    exception_list: Option<&Rc<Node>>,
    assoc: Option<&Token>,
    exception_variable: Option<&Rc<Node>>,
    then: Option<&Token>,
    compound_statement: Option<&Rc<Node>>,
) -> Location {
    let mut end_range = compound_statement.and_then(|node| expression(node));
    if end_range.is_none() && then.is_some() {
        end_range = location(then);
    }
    if end_range.is_none() && exception_variable.is_some() {
        end_range = optional_expression(exception_variable);
    }
    if end_range.is_none() && exception_list.is_some() {
        end_range = optional_expression(exception_list);
    }
    if end_range.is_none() {
        end_range = Some(keyword.range);
    }
    let mut map = Location::new(MapKind::RescueBody, join(Some(keyword.range), end_range));
    map.keyword = Some(keyword.range);
    map.assoc = location(assoc);
    map.begin = location(then);
    map
}

fn exception_handling_keyword_map(
    compound_statement: Option<&Rc<Node>>,
    keyword: Option<&Token>,
    bodies: &[Option<Rc<Node>>],
    else_token: Option<&Token>,
    else_body: Option<&Rc<Node>>,
) -> Location {
    let begin_range = match compound_statement {
        None => match keyword {
            None => bodies
                .first()
                .and_then(|body| optional_expression(body.as_ref())),
            Some(keyword) => Some(keyword.range),
        },
        Some(compound_statement) => expression(compound_statement),
    };
    let end_range = if else_token.is_some() {
        match else_body {
            None => location(else_token),
            Some(else_body) => expression(else_body),
        }
    } else if let Some(Some(last)) = bodies.last() {
        expression(last)
    } else {
        location(keyword)
    };
    let mut map = Location::new(MapKind::Condition, join(begin_range, end_range));
    map.keyword = location(keyword);
    map.else_ = location(else_token);
    map
}

fn guard_map(keyword: &Token, body: &Node) -> Location {
    let mut map = Location::new(
        MapKind::Keyword,
        join(Some(keyword.range), expression(body)),
    );
    map.keyword = Some(keyword.range);
    map
}

fn is_lowercase_variable_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first == '_' || first.is_lowercase() => {
            characters.all(|character| character == '_' || character.is_alphanumeric())
        }
        _ => false,
    }
}

fn is_numbered_parameter_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 2 && bytes[0] == b'_' && (b'1'..=b'9').contains(&bytes[1])
}

fn is_kwargs(node: &Node) -> bool {
    node.type_ == "hash" && node.location.begin.is_none() && node.location.end.is_none()
}

fn rewrite_hash_arguments_to_kwargs(arguments: &mut [Rc<Node>]) {
    let length = arguments.len();
    if length > 0 && is_kwargs(&arguments[length - 1]) {
        arguments[length - 1] = arguments[length - 1].updated(Some("kwargs"), None, None);
    } else if length > 1
        && arguments[length - 1].type_ == "block_pass"
        && is_kwargs(&arguments[length - 2])
    {
        arguments[length - 2] = arguments[length - 2].updated(Some("kwargs"), None, None);
    }
}

fn collapse_string_parts(parts: &[Rc<Node>]) -> bool {
    parts.len() == 1 && matches!(parts[0].type_, "str" | "dstr")
}

fn check_condition(condition: Rc<Node>) -> Rc<Node> {
    match condition.type_ {
        "begin" if condition.children.len() == 1 => {
            let checked = condition
                .children
                .last()
                .and_then(Child::node)
                .map(|child| node(check_condition(child.clone())))
                .unwrap_or(Child::Nil);
            condition.updated(None, Some(vec![checked]), None)
        }
        "and" | "or" => {
            let children = condition
                .children
                .iter()
                .map(|child| match child {
                    Child::Node(child) => node(check_condition(child.clone())),
                    other => other.clone(),
                })
                .collect();
            condition.updated(None, Some(children), None)
        }
        "irange" | "erange" => {
            let type_ = if condition.type_ == "irange" {
                "iflipflop"
            } else {
                "eflipflop"
            };
            let children = condition
                .children
                .iter()
                .map(|child| match child {
                    Child::Node(child) => node(check_condition(child.clone())),
                    other => other.clone(),
                })
                .collect();
            condition.updated(Some(type_), Some(children), None)
        }
        "regexp" => {
            let range = expression(&condition);
            build(
                "match_current_line",
                vec![node(condition)],
                expression_map(range),
            )
        }
        _ => condition,
    }
}

impl<'buffer> Builder<'buffer> {
    pub fn new(buffer: &'buffer Buffer) -> Builder<'buffer> {
        Builder {
            buffer,
            error: None,
        }
    }

    fn diagnostic(&mut self, message: String, range: Option<SourceRange>) {
        if self.error.is_none() {
            self.error = Some(SyntaxError {
                message,
                location: range.unwrap_or(SourceRange::new(0, 0)),
            });
        }
    }

    fn check_reserved_for_numparam(&mut self, name: &str, range: SourceRange) {
        if is_numbered_parameter_name(name) {
            self.diagnostic(
                format!("{name} is reserved for numbered parameter"),
                Some(range),
            );
        }
    }

    fn check_lvar_name(&mut self, name: &str, range: SourceRange) {
        if !is_lowercase_variable_name(name) {
            self.diagnostic(
                format!("`{name}' is not allowed as a local variable name"),
                Some(range),
            );
        }
    }

    fn validate_definee(&mut self, definee: Option<&Rc<Node>>) {
        if let Some(definee) = definee
            && matches!(
                definee.type_,
                "int" | "str" | "dstr" | "sym" | "dsym" | "regexp" | "array" | "hash"
            )
        {
            self.diagnostic(
                "cannot define a singleton method for a literal".to_string(),
                expression(definee),
            );
        }
    }

    pub fn nil(&mut self, token: &Token) -> Rc<Node> {
        build_leaf("nil", token_map(token))
    }

    pub fn true_(&mut self, token: &Token) -> Rc<Node> {
        build_leaf("true", token_map(token))
    }

    pub fn false_(&mut self, token: &Token) -> Rc<Node> {
        build_leaf("false", token_map(token))
    }

    pub fn self_(&mut self, token: &Token) -> Rc<Node> {
        build_leaf("self", token_map(token))
    }

    pub fn numeric(&mut self, kind: &'static str, value: Child, range: SourceRange) -> Rc<Node> {
        build(
            kind,
            vec![value],
            Location::new(MapKind::Operator, Some(range)),
        )
    }

    pub fn unary_num(&mut self, operator: SourceRange, numeric: Rc<Node>) -> Rc<Node> {
        let mut map = Location::new(
            MapKind::Operator,
            join(Some(operator), expression(&numeric)),
        );
        map.operator = Some(operator);
        numeric.updated(None, None, Some(map))
    }

    pub fn source_line(&mut self, token: &Token) -> Rc<Node> {
        build_leaf("__LINE__", token_map(token))
    }

    pub fn source_file(&mut self, token: &Token) -> Rc<Node> {
        build_leaf("__FILE__", token_map(token))
    }

    pub fn source_encoding(&mut self, token: &Token) -> Rc<Node> {
        build_leaf("__ENCODING__", token_map(token))
    }

    pub fn string_internal(&mut self, value: String, range: SourceRange) -> Rc<Node> {
        build("str", vec![Child::String(value)], unquoted_map(range))
    }

    pub fn string_compose(
        &mut self,
        begin: Option<&Token>,
        parts: Vec<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        if collapse_string_parts(&parts) {
            if begin.is_none() && end.is_none() {
                parts[0].clone()
            } else {
                let map = string_map(begin, &parts, end);
                build("str", parts[0].children.clone(), map)
            }
        } else {
            let map = string_map(begin, &parts, end);
            build("dstr", nodes(parts), map)
        }
    }

    pub fn character(&mut self, value: String, range: SourceRange) -> Rc<Node> {
        build("str", vec![Child::String(value)], prefix_string_map(range))
    }

    pub fn symbol(&mut self, value: String, range: SourceRange) -> Rc<Node> {
        build("sym", vec![symbol(value)], prefix_string_map(range))
    }

    pub fn symbol_internal(&mut self, value: String, range: SourceRange) -> Rc<Node> {
        build("sym", vec![symbol(value)], unquoted_map(range))
    }

    pub fn symbol_compose(
        &mut self,
        begin: Option<&Token>,
        parts: Vec<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        if collapse_string_parts(&parts) {
            let string = &parts[0];
            let value = match string.children.first() {
                Some(Child::String(value)) => symbol(value.clone()),
                Some(other) => other.clone(),
                None => Child::Nil,
            };
            let map = if begin.is_none() || end.is_none() {
                collection_map(begin, std::slice::from_ref(string), end)
            } else {
                collection_map(begin, &[], end)
            };
            build("sym", vec![value], map)
        } else {
            let map = collection_map(begin, &parts, end);
            build("dsym", nodes(parts), map)
        }
    }

    pub fn xstring_compose(
        &mut self,
        begin: Option<&Token>,
        parts: Vec<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        let map = string_map(begin, &parts, end);
        build("xstr", nodes(parts), map)
    }

    pub fn regexp_options(&mut self, token: &Token) -> Rc<Node> {
        let mut options: Vec<char> = token.value.chars().collect();
        options.sort_unstable();
        options.dedup();
        build(
            "regopt",
            options
                .into_iter()
                .map(|option| symbol(option.to_string()))
                .collect(),
            token_map(token),
        )
    }

    pub fn regexp_compose(
        &mut self,
        begin: &Token,
        parts: Vec<Rc<Node>>,
        end: &Token,
        options: Rc<Node>,
    ) -> Rc<Node> {
        let map = regexp_map(Some(begin), Some(end), &options);
        let mut children = nodes(parts);
        children.push(node(options));
        build("regexp", children, map)
    }

    pub fn array(
        &mut self,
        begin: Option<&Token>,
        elements: Vec<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        let map = collection_map(begin, &elements, end);
        build("array", nodes(elements), map)
    }

    pub fn splat(&mut self, star: &Token, argument: Option<Rc<Node>>) -> Rc<Node> {
        match argument {
            None => build_leaf("splat", unary_operator_map(star, None)),
            Some(argument) => {
                let map = unary_operator_map(star, Some(&argument));
                build("splat", vec![node(argument)], map)
            }
        }
    }

    pub fn pair(&mut self, key: Rc<Node>, assoc: &Token, value: Rc<Node>) -> Rc<Node> {
        let map = binary_operator_map(&key, Some(assoc), &value);
        build("pair", vec![node(key), node(value)], map)
    }

    pub fn pair_keyword(&mut self, key: &Token, value: Rc<Node>) -> Rc<Node> {
        let key_range = key.range;
        let key_map = unquoted_map(key_range.with_end_pos(key_range.end_pos - 1));
        let mut pair_map =
            Location::new(MapKind::Operator, join(Some(key_range), expression(&value)));
        pair_map.operator = Some(key_range.with_begin_pos(key_range.end_pos - 1));
        let key = build("sym", vec![symbol(key.value.clone())], key_map);
        build("pair", vec![node(key), node(value)], pair_map)
    }

    pub fn pair_quoted(
        &mut self,
        begin: &Token,
        parts: Vec<Rc<Node>>,
        end: &Token,
        value: Rc<Node>,
    ) -> Rc<Node> {
        let end_range = end.range;
        let quote = end_range
            .with_begin_pos(end_range.end_pos - 2)
            .with_end_pos(end_range.end_pos - 1);
        let mut pair_map = Location::new(
            MapKind::Operator,
            join(Some(begin.range), expression(&value)),
        );
        pair_map.operator = Some(end_range.with_begin_pos(end_range.end_pos - 1));
        let end = Token::new(end.value.clone(), quote);
        let key = self.symbol_compose(Some(begin), parts, Some(&end));
        build("pair", vec![node(key), node(value)], pair_map)
    }

    pub fn kwsplat(&mut self, double_star: &Token, argument: Option<Rc<Node>>) -> Rc<Node> {
        let map = unary_operator_map(double_star, argument.as_ref());
        build("kwsplat", vec![optional(argument)], map)
    }

    pub fn associate(
        &mut self,
        begin: Option<&Token>,
        pairs: Vec<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        let map = collection_map(begin, &pairs, end);
        build("hash", nodes(pairs), map)
    }

    pub fn range_inclusive(
        &mut self,
        left: Option<Rc<Node>>,
        operator: &Token,
        right: Option<Rc<Node>>,
    ) -> Rc<Node> {
        let map = range_map(left.as_ref(), operator, right.as_ref());
        build("irange", vec![optional(left), optional(right)], map)
    }

    pub fn range_exclusive(
        &mut self,
        left: Option<Rc<Node>>,
        operator: &Token,
        right: Option<Rc<Node>>,
    ) -> Rc<Node> {
        let map = range_map(left.as_ref(), operator, right.as_ref());
        build("erange", vec![optional(left), optional(right)], map)
    }

    pub fn ident(&mut self, token: &Token) -> Rc<Node> {
        build(
            "ident",
            vec![symbol(token.value.clone())],
            variable_map(token),
        )
    }

    pub fn ivar(&mut self, token: &Token) -> Rc<Node> {
        build(
            "ivar",
            vec![symbol(token.value.clone())],
            variable_map(token),
        )
    }

    pub fn gvar(&mut self, token: &Token) -> Rc<Node> {
        if token.value.starts_with("$0") && token.value.chars().count() > 2 {
            self.diagnostic(
                format!("`{}' is not allowed as a global variable name", token.value),
                Some(token.range),
            );
        }
        build(
            "gvar",
            vec![symbol(token.value.clone())],
            variable_map(token),
        )
    }

    pub fn cvar(&mut self, token: &Token) -> Rc<Node> {
        build(
            "cvar",
            vec![symbol(token.value.clone())],
            variable_map(token),
        )
    }

    pub fn back_ref(&mut self, token: &Token) -> Rc<Node> {
        build(
            "back_ref",
            vec![symbol(token.value.clone())],
            token_map(token),
        )
    }

    pub fn nth_ref(&mut self, number: u32, range: SourceRange) -> Rc<Node> {
        build(
            "nth_ref",
            vec![Child::Integer(number.to_string())],
            expression_map(Some(range)),
        )
    }

    pub fn accessible(&mut self, node: Rc<Node>) -> Rc<Node> {
        match node.type_ {
            "__FILE__" => build(
                "str",
                vec![Child::String(self.buffer.name.clone())],
                node.location.clone(),
            ),
            "__LINE__" => {
                let line = expression(&node).map_or(0, |range| self.buffer.line(range.begin_pos));
                build(
                    "int",
                    vec![Child::Integer(line.to_string())],
                    node.location.clone(),
                )
            }
            "__ENCODING__" => {
                let encoding = build(
                    "const",
                    vec![Child::Nil, symbol("Encoding")],
                    Location::new(MapKind::Map, None),
                );
                build(
                    "const",
                    vec![Child::Node(encoding), symbol("UTF_8")],
                    node.location.clone(),
                )
            }
            _ => node,
        }
    }

    pub fn const_(&mut self, name: &Token) -> Rc<Node> {
        build(
            "const",
            vec![Child::Nil, symbol(name.value.clone())],
            constant_map(None, None, name),
        )
    }

    pub fn const_global(&mut self, double_colon: &Token, name: &Token) -> Rc<Node> {
        let cbase = build_leaf("cbase", token_map(double_colon));
        let map = constant_map(Some(&cbase), Some(double_colon), name);
        build("const", vec![node(cbase), symbol(name.value.clone())], map)
    }

    pub fn const_fetch(
        &mut self,
        scope: Option<Rc<Node>>,
        double_colon: &Token,
        name: &Token,
    ) -> Rc<Node> {
        let map = constant_map(scope.as_ref(), Some(double_colon), name);
        build(
            "const",
            vec![optional(scope), symbol(name.value.clone())],
            map,
        )
    }

    pub fn assignable(&mut self, node: Rc<Node>) -> Rc<Node> {
        match node.type_ {
            "cvar" => node.updated(Some("cvasgn"), None, None),
            "ivar" => node.updated(Some("ivasgn"), None, None),
            "gvar" => node.updated(Some("gvasgn"), None, None),
            "const" => node.updated(Some("casgn"), None, None),
            "ident" => {
                if let (Some(name), Some(range)) = (node.child_symbol(0), expression(&node)) {
                    let name = name.to_string();
                    self.check_reserved_for_numparam(&name, range);
                }
                node.updated(Some("lvasgn"), None, None)
            }
            "match_var" => {
                if let (Some(name), Some(range)) = (node.child_symbol(0), expression(&node)) {
                    let name = name.to_string();
                    self.check_reserved_for_numparam(&name, range);
                }
                node
            }
            "nil" | "self" | "true" | "false" | "__FILE__" | "__LINE__" | "__ENCODING__" => {
                self.diagnostic("cannot assign to a keyword".to_string(), expression(&node));
                node
            }
            "back_ref" | "nth_ref" => {
                self.diagnostic(
                    "cannot assign to a back-reference variable".to_string(),
                    expression(&node),
                );
                node
            }
            _ => node,
        }
    }

    pub fn assign(
        &mut self,
        left: Rc<Node>,
        equal: Option<&Token>,
        right: Option<Rc<Node>>,
    ) -> Rc<Node> {
        let mut map = left.location.clone();
        map.operator = location(equal);
        map.expression = join(expression(&left), optional_expression(right.as_ref()));
        let mut children = left.children.clone();
        children.push(optional(right));
        left.updated(None, Some(children), Some(map))
    }

    pub fn op_assign(
        &mut self,
        left: Rc<Node>,
        operator: &Token,
        right: Option<Rc<Node>>,
    ) -> Rc<Node> {
        let mut map = left.location.clone();
        map.operator = Some(operator.range);
        map.expression = join(expression(&left), optional_expression(right.as_ref()));
        match operator.value.as_str() {
            "&&" => build("and_asgn", vec![node(left), optional(right)], map),
            "||" => build("or_asgn", vec![node(left), optional(right)], map),
            other => build(
                "op_asgn",
                vec![node(left), symbol(other), optional(right)],
                map,
            ),
        }
    }

    pub fn multi_lhs(
        &mut self,
        begin: Option<&Token>,
        items: Vec<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        let map = collection_map(begin, &items, end);
        build("mlhs", nodes(items), map)
    }

    pub fn multi_assign(&mut self, left: Rc<Node>, equal: &Token, right: Rc<Node>) -> Rc<Node> {
        let map = binary_operator_map(&left, Some(equal), &right);
        build("masgn", vec![node(left), node(right)], map)
    }

    pub fn def_class(
        &mut self,
        class: &Token,
        name: Option<Rc<Node>>,
        less_than: Option<&Token>,
        superclass: Option<Rc<Node>>,
        body: Option<Rc<Node>>,
        end: &Token,
    ) -> Rc<Node> {
        let map = module_definition_map(class, name.as_ref(), less_than, end);
        build(
            "class",
            vec![optional(name), optional(superclass), optional(body)],
            map,
        )
    }

    pub fn def_sclass(
        &mut self,
        class: &Token,
        left_shift: &Token,
        expression: Option<Rc<Node>>,
        body: Option<Rc<Node>>,
        end: &Token,
    ) -> Rc<Node> {
        let map = module_definition_map(class, None, Some(left_shift), end);
        build("sclass", vec![optional(expression), optional(body)], map)
    }

    pub fn def_module(
        &mut self,
        module: &Token,
        name: Option<Rc<Node>>,
        body: Option<Rc<Node>>,
        end: &Token,
    ) -> Rc<Node> {
        let map = module_definition_map(module, name.as_ref(), None, end);
        build("module", vec![optional(name), optional(body)], map)
    }

    pub fn def_method(
        &mut self,
        def: &Token,
        name: &Token,
        arguments: Rc<Node>,
        body: Option<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        self.check_reserved_for_numparam(&name.value, name.range);
        let map = definition_map(def, None, name, end);
        build(
            "def",
            vec![symbol(name.value.clone()), node(arguments), optional(body)],
            map,
        )
    }

    pub fn def_endless_method(
        &mut self,
        def: &Token,
        name: &Token,
        arguments: Rc<Node>,
        assignment: Option<&Token>,
        body: Option<Rc<Node>>,
    ) -> Rc<Node> {
        self.check_reserved_for_numparam(&name.value, name.range);
        let map = endless_definition_map(def, None, name, assignment, body.as_ref());
        build(
            "def",
            vec![symbol(name.value.clone()), node(arguments), optional(body)],
            map,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn def_singleton(
        &mut self,
        def: &Token,
        definee: Option<Rc<Node>>,
        dot: Option<&Token>,
        name: &Token,
        arguments: Rc<Node>,
        body: Option<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        self.validate_definee(definee.as_ref());
        self.check_reserved_for_numparam(&name.value, name.range);
        let map = definition_map(def, dot, name, end);
        build(
            "defs",
            vec![
                optional(definee),
                symbol(name.value.clone()),
                node(arguments),
                optional(body),
            ],
            map,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn def_endless_singleton(
        &mut self,
        def: &Token,
        definee: Option<Rc<Node>>,
        dot: Option<&Token>,
        name: &Token,
        arguments: Rc<Node>,
        assignment: Option<&Token>,
        body: Option<Rc<Node>>,
    ) -> Rc<Node> {
        self.validate_definee(definee.as_ref());
        self.check_reserved_for_numparam(&name.value, name.range);
        let map = endless_definition_map(def, dot, name, assignment, body.as_ref());
        build(
            "defs",
            vec![
                optional(definee),
                symbol(name.value.clone()),
                node(arguments),
                optional(body),
            ],
            map,
        )
    }

    pub fn undef_method(&mut self, undef: &Token, names: Vec<Rc<Node>>) -> Rc<Node> {
        let children = nodes(names);
        let map = keyword_map(undef, None, &children, None);
        build("undef", children, map)
    }

    pub fn alias(
        &mut self,
        alias: &Token,
        to: Option<Rc<Node>>,
        from: Option<Rc<Node>>,
    ) -> Rc<Node> {
        let children = vec![optional(to), optional(from)];
        let map = keyword_map(alias, None, &children, None);
        build("alias", children, map)
    }

    pub fn args(
        &mut self,
        begin: Option<&Token>,
        arguments: Vec<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        let rest = arguments
            .iter()
            .rev()
            .find(|argument| argument.type_ == "restarg");
        let forward = arguments
            .iter()
            .rev()
            .find(|argument| argument.type_ == "forward_arg");
        if let (Some(forward), Some(_)) = (forward, rest) {
            let range = expression(forward);
            self.diagnostic("... after rest argument".to_string(), range);
        }
        let map = collection_map(begin, &arguments, end);
        build("args", nodes(arguments), map)
    }

    pub fn forward_arg(&mut self, dots: &Token) -> Rc<Node> {
        build_leaf("forward_arg", token_map(dots))
    }

    pub fn arg(&mut self, name: &Token) -> Rc<Node> {
        self.check_reserved_for_numparam(&name.value, name.range);
        build("arg", vec![symbol(name.value.clone())], variable_map(name))
    }

    pub fn optarg(&mut self, name: &Token, equal: &Token, value: Rc<Node>) -> Rc<Node> {
        self.check_reserved_for_numparam(&name.value, name.range);
        let mut map = variable_map(name);
        map.operator = Some(equal.range);
        map.expression = join(Some(name.range), expression(&value));
        build("optarg", vec![symbol(name.value.clone()), node(value)], map)
    }

    pub fn restarg(&mut self, star: &Token, name: Option<&Token>) -> Rc<Node> {
        match name {
            Some(name) => {
                self.check_reserved_for_numparam(&name.value, name.range);
                build(
                    "restarg",
                    vec![symbol(name.value.clone())],
                    argument_prefix_map(star, Some(name)),
                )
            }
            None => build_leaf("restarg", argument_prefix_map(star, None)),
        }
    }

    pub fn kwarg(&mut self, name: &Token) -> Rc<Node> {
        self.check_reserved_for_numparam(&name.value, name.range);
        build(
            "kwarg",
            vec![symbol(name.value.clone())],
            keyword_argument_map(name, None),
        )
    }

    pub fn kwoptarg(&mut self, name: &Token, value: Rc<Node>) -> Rc<Node> {
        self.check_reserved_for_numparam(&name.value, name.range);
        let map = keyword_argument_map(name, Some(&value));
        build(
            "kwoptarg",
            vec![symbol(name.value.clone()), node(value)],
            map,
        )
    }

    pub fn kwrestarg(&mut self, double_star: &Token, name: Option<&Token>) -> Rc<Node> {
        match name {
            Some(name) => {
                self.check_reserved_for_numparam(&name.value, name.range);
                build(
                    "kwrestarg",
                    vec![symbol(name.value.clone())],
                    argument_prefix_map(double_star, Some(name)),
                )
            }
            None => build_leaf("kwrestarg", argument_prefix_map(double_star, None)),
        }
    }

    pub fn kwnilarg(&mut self, double_star: &Token, nil: &Token) -> Rc<Node> {
        build_leaf("kwnilarg", argument_prefix_map(double_star, Some(nil)))
    }

    pub fn shadowarg(&mut self, name: &Token) -> Rc<Node> {
        self.check_reserved_for_numparam(&name.value, name.range);
        build(
            "shadowarg",
            vec![symbol(name.value.clone())],
            variable_map(name),
        )
    }

    pub fn blockarg(&mut self, ampersand: &Token, name: Option<&Token>) -> Rc<Node> {
        if let Some(name) = name {
            self.check_reserved_for_numparam(&name.value, name.range);
        }
        let argument_name = name.map_or(Child::Nil, |name| symbol(name.value.clone()));
        build(
            "blockarg",
            vec![argument_name],
            argument_prefix_map(ampersand, name),
        )
    }

    pub fn procarg0(&mut self, argument: Rc<Node>) -> Rc<Node> {
        argument.updated(Some("procarg0"), None, None)
    }

    pub fn forwarded_args(&mut self, dots: &Token) -> Rc<Node> {
        build_leaf("forwarded_args", token_map(dots))
    }

    pub fn forwarded_restarg(&mut self, star: &Token) -> Rc<Node> {
        build_leaf("forwarded_restarg", token_map(star))
    }

    pub fn forwarded_kwrestarg(&mut self, double_star: &Token) -> Rc<Node> {
        build_leaf("forwarded_kwrestarg", token_map(double_star))
    }

    pub fn call_method(
        &mut self,
        receiver: Option<Rc<Node>>,
        dot: Option<&Token>,
        selector: Option<&Token>,
        left_parenthesis: Option<&Token>,
        mut arguments: Vec<Rc<Node>>,
        right_parenthesis: Option<&Token>,
    ) -> Rc<Node> {
        let type_ = call_type_for_dot(dot);
        rewrite_hash_arguments_to_kwargs(&mut arguments);
        let map = send_map(
            receiver.as_ref(),
            dot,
            selector,
            left_parenthesis,
            &arguments,
            right_parenthesis,
        );
        let name = selector.map_or_else(|| "call".to_string(), |selector| selector.value.clone());
        let mut children = vec![optional(receiver), symbol(name)];
        children.extend(nodes(arguments));
        build(type_, children, map)
    }

    pub fn call_lambda(&mut self, lambda: &Token) -> Rc<Node> {
        build_leaf("lambda", expression_map(Some(lambda.range)))
    }

    pub fn block(
        &mut self,
        method_call: Rc<Node>,
        begin: &Token,
        arguments: BlockArguments,
        body: Option<Rc<Node>>,
        end: &Token,
    ) -> Rc<Node> {
        if method_call.type_ == "yield" {
            self.diagnostic(
                "block given to yield".to_string(),
                method_call.location.keyword,
            );
        }
        if let Some(Child::Node(last_argument)) =
            method_call.children.get(2..).and_then(<[Child]>::last)
            && matches!(last_argument.type_, "block_pass" | "forwarded_args")
        {
            self.diagnostic(
                "both block argument and literal block are passed".to_string(),
                expression(last_argument),
            );
        }
        let (block_type, arguments) = match arguments {
            BlockArguments::It => ("itblock", symbol("it")),
            BlockArguments::Numbered(maximum) => ("numblock", Child::Integer(maximum.to_string())),
            BlockArguments::Arguments(arguments) => ("block", node(arguments)),
        };
        if matches!(
            method_call.type_,
            "send" | "csend" | "index" | "super" | "zsuper" | "lambda"
        ) {
            let map = block_map(expression(&method_call), Some(begin), Some(end));
            build(
                block_type,
                vec![node(method_call), arguments, optional(body)],
                map,
            )
        } else {
            let actual_send = method_call.child_node(0).cloned();
            let map = block_map(
                optional_expression(actual_send.as_ref()),
                Some(begin),
                Some(end),
            );
            let block = build(
                block_type,
                vec![optional(actual_send), arguments, optional(body)],
                map,
            );
            let location = method_call.location.with_expression(
                join(expression(&method_call), expression(&block))
                    .unwrap_or(SourceRange::new(0, 0)),
            );
            build(method_call.type_, vec![node(block)], location)
        }
    }

    pub fn block_pass(&mut self, ampersand: &Token, argument: Option<Rc<Node>>) -> Rc<Node> {
        let map = unary_operator_map(ampersand, argument.as_ref());
        build("block_pass", vec![optional(argument)], map)
    }

    pub fn attr_asgn(
        &mut self,
        receiver: Option<Rc<Node>>,
        dot: Option<&Token>,
        selector: &Token,
    ) -> Rc<Node> {
        let method_name = format!("{}=", selector.value);
        let type_ = call_type_for_dot(dot);
        let map = send_map(receiver.as_ref(), dot, Some(selector), None, &[], None);
        build(type_, vec![optional(receiver), symbol(method_name)], map)
    }

    pub fn index(
        &mut self,
        receiver: Option<Rc<Node>>,
        left_bracket: Option<&Token>,
        mut indexes: Vec<Rc<Node>>,
        right_bracket: Option<&Token>,
    ) -> Rc<Node> {
        rewrite_hash_arguments_to_kwargs(&mut indexes);
        let map = send_index_map(receiver.as_ref(), left_bracket, right_bracket);
        let mut children = vec![optional(receiver), symbol("[]")];
        children.extend(nodes(indexes));
        build("send", children, map)
    }

    pub fn index_asgn(
        &mut self,
        receiver: Option<Rc<Node>>,
        left_bracket: Option<&Token>,
        mut indexes: Vec<Rc<Node>>,
        right_bracket: Option<&Token>,
    ) -> Rc<Node> {
        rewrite_hash_arguments_to_kwargs(&mut indexes);
        let map = send_index_map(receiver.as_ref(), left_bracket, right_bracket);
        let mut children = vec![optional(receiver), symbol("[]=")];
        children.extend(nodes(indexes));
        build("send", children, map)
    }

    pub fn match_op(
        &mut self,
        receiver: Rc<Node>,
        match_token: &Token,
        argument: Option<Rc<Node>>,
    ) -> Rc<Node> {
        let map = send_binary_operator_map(&receiver, match_token, argument.as_ref());
        let static_regexp = receiver.type_ == "regexp"
            && receiver.children[..receiver.children.len().saturating_sub(1)]
                .iter()
                .all(|child| child.node().is_some_and(|child| child.type_ == "str"));
        if static_regexp {
            build(
                "match_with_lvasgn",
                vec![node(receiver), optional(argument)],
                map,
            )
        } else {
            build(
                "send",
                vec![node(receiver), symbol("=~"), optional(argument)],
                map,
            )
        }
    }

    pub fn not_op(
        &mut self,
        not: &Token,
        begin: Option<&Token>,
        receiver: Option<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        match receiver {
            None => {
                let nil_node = build_leaf("begin", collection_map(begin, &[], end));
                let map = send_unary_operator_map(not, Some(&nil_node));
                build("send", vec![node(nil_node), symbol("!")], map)
            }
            Some(receiver) => {
                let map = send_map(
                    None,
                    None,
                    Some(not),
                    begin,
                    std::slice::from_ref(&receiver),
                    end,
                );
                build(
                    "send",
                    vec![node(check_condition(receiver)), symbol("!")],
                    map,
                )
            }
        }
    }

    pub fn logical_op(
        &mut self,
        type_: &'static str,
        left: Rc<Node>,
        operator: &Token,
        right: Rc<Node>,
    ) -> Rc<Node> {
        let map = binary_operator_map(&left, Some(operator), &right);
        build(type_, vec![node(left), node(right)], map)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn condition(
        &mut self,
        condition_token: &Token,
        condition: Rc<Node>,
        then: Option<&Token>,
        if_true: Option<Rc<Node>>,
        else_token: Option<&Token>,
        if_false: Option<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        let map = condition_map(
            condition_token,
            Some(&condition),
            then,
            if_true.as_ref(),
            else_token,
            if_false.as_ref(),
            end,
        );
        build(
            "if",
            vec![
                node(check_condition(condition)),
                optional(if_true),
                optional(if_false),
            ],
            map,
        )
    }

    pub fn condition_mod(
        &mut self,
        if_true: Option<Rc<Node>>,
        if_false: Option<Rc<Node>>,
        condition_token: &Token,
        condition: Rc<Node>,
    ) -> Rc<Node> {
        let map = keyword_modifier_map(
            if_true.as_ref().or(if_false.as_ref()),
            condition_token,
            &condition,
        );
        build(
            "if",
            vec![
                node(check_condition(condition)),
                optional(if_true),
                optional(if_false),
            ],
            map,
        )
    }

    pub fn ternary(
        &mut self,
        condition: Rc<Node>,
        question: Option<&Token>,
        if_true: Option<Rc<Node>>,
        colon: Option<&Token>,
        if_false: Option<Rc<Node>>,
    ) -> Rc<Node> {
        let mut map = Location::new(
            MapKind::Ternary,
            join(
                expression(&condition),
                optional_expression(if_false.as_ref()),
            ),
        );
        map.question = location(question);
        map.colon = location(colon);
        build(
            "if",
            vec![
                node(check_condition(condition)),
                optional(if_true),
                optional(if_false),
            ],
            map,
        )
    }

    pub fn when(
        &mut self,
        when: &Token,
        patterns: Vec<Rc<Node>>,
        then: Option<&Token>,
        body: Option<Rc<Node>>,
    ) -> Rc<Node> {
        let mut children = nodes(patterns);
        children.push(optional(body));
        let map = keyword_map(when, then, &children, None);
        build("when", children, map)
    }

    pub fn case(
        &mut self,
        case: &Token,
        expression: Option<Rc<Node>>,
        when_bodies: Vec<Rc<Node>>,
        else_token: Option<&Token>,
        else_body: Option<Rc<Node>>,
        end: &Token,
    ) -> Rc<Node> {
        let map = condition_map(
            case,
            expression.as_ref(),
            None,
            None,
            else_token,
            else_body.as_ref(),
            Some(end),
        );
        let mut children = vec![optional(expression)];
        children.extend(nodes(when_bodies));
        children.push(optional(else_body));
        build("case", children, map)
    }

    pub fn loop_(
        &mut self,
        type_: &'static str,
        keyword: &Token,
        condition: Rc<Node>,
        do_token: Option<&Token>,
        body: Option<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        let map = keyword_map(keyword, do_token, &[], end);
        build(
            type_,
            vec![node(check_condition(condition)), optional(body)],
            map,
        )
    }

    pub fn loop_mod(
        &mut self,
        type_: &'static str,
        body: Option<Rc<Node>>,
        keyword: &Token,
        condition: Rc<Node>,
    ) -> Rc<Node> {
        let type_ = match (body.as_ref().map(|body| body.type_), type_) {
            (Some("kwbegin"), "while") => "while_post",
            (Some("kwbegin"), "until") => "until_post",
            _ => type_,
        };
        let map = keyword_modifier_map(body.as_ref(), keyword, &condition);
        build(
            type_,
            vec![node(check_condition(condition)), optional(body)],
            map,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn for_(
        &mut self,
        for_token: &Token,
        iterator: Option<Rc<Node>>,
        in_token: &Token,
        iteratee: Option<Rc<Node>>,
        do_token: Option<&Token>,
        body: Option<Rc<Node>>,
        end: &Token,
    ) -> Rc<Node> {
        let mut map = Location::new(MapKind::For, Some(for_token.range.join(end.range)));
        map.keyword = Some(for_token.range);
        map.in_ = Some(in_token.range);
        map.begin = location(do_token);
        map.end = Some(end.range);
        build(
            "for",
            vec![optional(iterator), optional(iteratee), optional(body)],
            map,
        )
    }

    pub fn keyword_cmd(
        &mut self,
        type_: &'static str,
        keyword: &Token,
        left_parenthesis: Option<&Token>,
        mut arguments: Vec<Rc<Node>>,
        right_parenthesis: Option<&Token>,
    ) -> Rc<Node> {
        if type_ == "yield"
            && let Some(last) = arguments.last()
            && last.type_ == "block_pass"
        {
            let range = Some(keyword.range);
            self.diagnostic("block given to yield".to_string(), range);
        }
        if type_ == "yield" || type_ == "super" {
            rewrite_hash_arguments_to_kwargs(&mut arguments);
        }
        let children = nodes(arguments);
        let map = keyword_map(keyword, left_parenthesis, &children, right_parenthesis);
        build(type_, children, map)
    }

    pub fn preexe(
        &mut self,
        type_: &'static str,
        keyword: &Token,
        left_brace: &Token,
        compound_statement: Option<Rc<Node>>,
        right_brace: &Token,
    ) -> Rc<Node> {
        let map = keyword_map(keyword, Some(left_brace), &[], Some(right_brace));
        build(type_, vec![optional(compound_statement)], map)
    }

    pub fn rescue_body(
        &mut self,
        rescue: &Token,
        exception_list: Option<Rc<Node>>,
        assoc: Option<&Token>,
        exception_variable: Option<Rc<Node>>,
        then: Option<&Token>,
        compound_statement: Option<Rc<Node>>,
    ) -> Rc<Node> {
        let map = rescue_body_map(
            rescue,
            exception_list.as_ref(),
            assoc,
            exception_variable.as_ref(),
            then,
            compound_statement.as_ref(),
        );
        build(
            "resbody",
            vec![
                optional(exception_list),
                optional(exception_variable),
                optional(compound_statement),
            ],
            map,
        )
    }

    pub fn begin_body(
        &mut self,
        compound_statement: Option<Rc<Node>>,
        rescue_bodies: Vec<Rc<Node>>,
        else_token: Option<&Token>,
        else_body: Option<Rc<Node>>,
        ensure_token: Option<&Token>,
        ensure_body: Option<Rc<Node>>,
    ) -> Option<Rc<Node>> {
        let mut compound_statement = compound_statement;
        if !rescue_bodies.is_empty() {
            let bodies: Vec<Option<Rc<Node>>> = rescue_bodies.iter().cloned().map(Some).collect();
            let map = if else_token.is_some() {
                exception_handling_keyword_map(
                    compound_statement.as_ref(),
                    None,
                    &bodies,
                    else_token,
                    else_body.as_ref(),
                )
            } else {
                exception_handling_keyword_map(
                    compound_statement.as_ref(),
                    None,
                    &bodies,
                    None,
                    None,
                )
            };
            let mut children = vec![optional(compound_statement.take())];
            children.extend(nodes(rescue_bodies));
            children.push(if else_token.is_some() {
                optional(else_body)
            } else {
                Child::Nil
            });
            compound_statement = Some(build("rescue", children, map));
        } else if let Some(else_token) = else_token {
            let mut statements = Vec::new();
            if let Some(compound_statement) = compound_statement.take() {
                if compound_statement.type_ == "begin" {
                    statements.extend(compound_statement.children.iter().cloned());
                } else {
                    statements.push(node(compound_statement));
                }
            }
            let else_parts: Vec<Rc<Node>> = else_body.iter().cloned().collect();
            let else_map = collection_map(Some(else_token), &else_parts, None);
            statements.push(node(build("begin", vec![optional(else_body)], else_map)));
            let statement_nodes: Vec<Rc<Node>> = statements
                .iter()
                .filter_map(|statement| statement.node().cloned())
                .collect();
            let map = collection_map(None, &statement_nodes, None);
            compound_statement = Some(build("begin", statements, map));
        }
        if let Some(ensure_token) = ensure_token {
            let map = exception_handling_keyword_map(
                compound_statement.as_ref(),
                Some(ensure_token),
                std::slice::from_ref(&ensure_body),
                None,
                None,
            );
            compound_statement = Some(build(
                "ensure",
                vec![optional(compound_statement), optional(ensure_body)],
                map,
            ));
        }
        compound_statement
    }

    pub fn compstmt(&mut self, statements: Vec<Rc<Node>>) -> Option<Rc<Node>> {
        match statements.len() {
            0 => None,
            1 => statements.into_iter().next(),
            _ => {
                let map = collection_map(None, &statements, None);
                Some(build("begin", nodes(statements), map))
            }
        }
    }

    pub fn begin(&mut self, begin: &Token, body: Option<Rc<Node>>, end: &Token) -> Rc<Node> {
        match body {
            None => build_leaf("begin", collection_map(Some(begin), &[], Some(end))),
            Some(body)
                if body.type_ == "mlhs"
                    || (body.type_ == "begin"
                        && body.location.begin.is_none()
                        && body.location.end.is_none()) =>
            {
                let children: Vec<Rc<Node>> = body
                    .children
                    .iter()
                    .filter_map(|child| child.node().cloned())
                    .collect();
                let map = collection_map(Some(begin), &children, Some(end));
                build(body.type_, body.children.clone(), map)
            }
            Some(body) => {
                let map = collection_map(Some(begin), std::slice::from_ref(&body), Some(end));
                build("begin", vec![node(body)], map)
            }
        }
    }

    pub fn begin_keyword(
        &mut self,
        begin: &Token,
        body: Option<Rc<Node>>,
        end: Option<&Token>,
    ) -> Rc<Node> {
        match body {
            None => build_leaf("kwbegin", collection_map(Some(begin), &[], end)),
            Some(body)
                if body.type_ == "begin"
                    && body.location.begin.is_none()
                    && body.location.end.is_none() =>
            {
                let children: Vec<Rc<Node>> = body
                    .children
                    .iter()
                    .filter_map(|child| child.node().cloned())
                    .collect();
                let map = collection_map(Some(begin), &children, end);
                build("kwbegin", body.children.clone(), map)
            }
            Some(body) => {
                let map = collection_map(Some(begin), std::slice::from_ref(&body), end);
                build("kwbegin", vec![node(body)], map)
            }
        }
    }

    pub fn case_match(
        &mut self,
        case: &Token,
        expression: Option<Rc<Node>>,
        in_bodies: Vec<Rc<Node>>,
        else_token: Option<&Token>,
        else_body: Option<Rc<Node>>,
        end: &Token,
    ) -> Rc<Node> {
        let else_body = match (else_token, else_body) {
            (Some(else_token), None) => Some(build_leaf("empty_else", token_map(else_token))),
            (_, else_body) => else_body,
        };
        let map = condition_map(
            case,
            expression.as_ref(),
            None,
            None,
            else_token,
            else_body.as_ref(),
            Some(end),
        );
        let mut children = vec![optional(expression)];
        children.extend(nodes(in_bodies));
        children.push(optional(else_body));
        build("case_match", children, map)
    }

    pub fn match_pattern(
        &mut self,
        type_: &'static str,
        left: Rc<Node>,
        operator: &Token,
        right: Rc<Node>,
    ) -> Rc<Node> {
        let map = binary_operator_map(&left, Some(operator), &right);
        build(type_, vec![node(left), node(right)], map)
    }

    pub fn in_pattern(
        &mut self,
        in_token: &Token,
        pattern: Option<Rc<Node>>,
        guard: Option<Rc<Node>>,
        then: Option<&Token>,
        body: Option<Rc<Node>>,
    ) -> Rc<Node> {
        let children = vec![optional(pattern), optional(guard), optional(body)];
        let compact: Vec<Child> = children
            .iter()
            .filter(|child| !child.is_nil())
            .cloned()
            .collect();
        let map = keyword_map(in_token, then, &compact, None);
        build("in_pattern", children, map)
    }

    pub fn guard(&mut self, type_: &'static str, keyword: &Token, body: Rc<Node>) -> Rc<Node> {
        let map = guard_map(keyword, &body);
        build(type_, vec![node(body)], map)
    }

    pub fn match_var(&mut self, name: &Token) -> Rc<Node> {
        self.check_lvar_name(&name.value, name.range);
        build(
            "match_var",
            vec![symbol(name.value.clone())],
            variable_map(name),
        )
    }

    pub fn match_hash_var(&mut self, name: &Token) -> Rc<Node> {
        let expression_range = name.range;
        let name_range = expression_range.with_end_pos(expression_range.end_pos - 1);
        self.check_lvar_name(&name.value, name_range);
        let mut map = Location::new(MapKind::Variable, Some(expression_range));
        map.name = Some(name_range);
        build("match_var", vec![symbol(name.value.clone())], map)
    }

    pub fn match_hash_var_from_str(
        &mut self,
        begin: &Token,
        strings: Vec<Rc<Node>>,
        end: &Token,
    ) -> Option<Rc<Node>> {
        let interpolation_range = Some(begin.range.join(end.range));
        if strings.len() > 1 {
            self.diagnostic(
                "symbol literal with interpolation is not allowed".to_string(),
                interpolation_range,
            );
        }
        let string = strings.into_iter().next()?;
        match string.type_ {
            "str" => {
                let name = string.child(0).string().unwrap_or_default().to_string();
                let mut name_range = expression(&string)?;
                self.check_lvar_name(&name, name_range);
                if let Some(begin_range) = string.location.begin {
                    name_range =
                        name_range.with_begin_pos(name_range.begin_pos + begin_range.size());
                }
                if let Some(end_range) = string.location.end {
                    name_range = name_range.with_end_pos(name_range.end_pos - end_range.size());
                }
                let expression_range = join(
                    join(Some(begin.range), expression(&string)),
                    Some(end.range),
                );
                let mut map = Location::new(MapKind::Variable, expression_range);
                map.name = Some(name_range);
                Some(build("match_var", vec![symbol(name)], map))
            }
            "begin" => {
                let children = string
                    .children
                    .iter()
                    .filter_map(|child| child.node().cloned())
                    .collect();
                self.match_hash_var_from_str(begin, children, end)
            }
            _ => {
                self.diagnostic(
                    "symbol literal with interpolation is not allowed".to_string(),
                    interpolation_range,
                );
                None
            }
        }
    }

    pub fn match_rest(&mut self, star: &Token, name: Option<&Token>) -> Rc<Node> {
        match name {
            None => build_leaf("match_rest", unary_operator_map(star, None)),
            Some(name) => {
                let name = self.match_var(name);
                let map = unary_operator_map(star, Some(&name));
                build("match_rest", vec![node(name)], map)
            }
        }
    }

    pub fn hash_pattern(
        &mut self,
        left_brace: Option<&Token>,
        keyword_arguments: Vec<Rc<Node>>,
        right_brace: Option<&Token>,
    ) -> Rc<Node> {
        let map = collection_map(left_brace, &keyword_arguments, right_brace);
        build("hash_pattern", nodes(keyword_arguments), map)
    }

    pub fn array_pattern(
        &mut self,
        left_bracket: Option<&Token>,
        elements: Vec<Rc<Node>>,
        right_bracket: Option<&Token>,
    ) -> Rc<Node> {
        let mut trailing_comma = false;
        let node_elements: Vec<Child> = elements
            .iter()
            .map(|element| {
                if element.type_ == "match_with_trailing_comma" {
                    trailing_comma = true;
                    element.child(0).clone()
                } else {
                    trailing_comma = false;
                    node(element.clone())
                }
            })
            .collect();
        let node_type = if trailing_comma {
            "array_pattern_with_tail"
        } else {
            "array_pattern"
        };
        let map = collection_map(left_bracket, &elements, right_bracket);
        build(node_type, node_elements, map)
    }

    pub fn find_pattern(
        &mut self,
        left_bracket: Option<&Token>,
        elements: Vec<Rc<Node>>,
        right_bracket: Option<&Token>,
    ) -> Rc<Node> {
        let map = collection_map(left_bracket, &elements, right_bracket);
        build("find_pattern", nodes(elements), map)
    }

    pub fn match_with_trailing_comma(&mut self, matched: Rc<Node>, comma: &Token) -> Rc<Node> {
        let map = expression_map(join(expression(&matched), Some(comma.range)));
        build("match_with_trailing_comma", vec![node(matched)], map)
    }

    pub fn const_pattern(
        &mut self,
        constant: Rc<Node>,
        left_delimiter: Option<&Token>,
        pattern: Rc<Node>,
        right_delimiter: Option<&Token>,
    ) -> Rc<Node> {
        let mut map = Location::new(
            MapKind::Collection,
            join(expression(&constant), location(right_delimiter)),
        );
        map.begin = location(left_delimiter);
        map.end = location(right_delimiter);
        build("const_pattern", vec![node(constant), node(pattern)], map)
    }

    pub fn pin(&mut self, pin: &Token, variable: Option<Rc<Node>>) -> Rc<Node> {
        let map = send_unary_operator_map(pin, variable.as_ref());
        build("pin", vec![optional(variable)], map)
    }

    pub fn binary_pattern(
        &mut self,
        type_: &'static str,
        left: Rc<Node>,
        operator: &Token,
        right: Rc<Node>,
    ) -> Rc<Node> {
        let map = binary_operator_map(&left, Some(operator), &right);
        build(type_, vec![node(left), node(right)], map)
    }

    pub fn match_nil_pattern(&mut self, double_star: &Token, nil: &Token) -> Rc<Node> {
        build_leaf(
            "match_nil_pattern",
            argument_prefix_map(double_star, Some(nil)),
        )
    }
}

fn call_type_for_dot(dot: Option<&Token>) -> &'static str {
    match dot {
        Some(dot) if dot.value == "anddot" => "csend",
        _ => "send",
    }
}
