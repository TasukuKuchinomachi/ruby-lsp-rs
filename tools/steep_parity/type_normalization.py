import re

NORMALIZATION_RULES = [
    "型名の先頭の `::` を除去する (`::Array[::Integer]` → `Array[Integer]`)",
    "`A?` と `(A | nil)` と `nil | A` を同一視する (union を展開して nil を取り出し、最後に `?` で表す)",
    "union / intersection の要素の順序を無視する (要素を正規化した文字列でソート)、重複を除く、入れ子の union を平坦化する",
    "`bool` と `true | false` を同一視する",
    "グループ化のための括弧を除く (`(A)` → `A`)",
    "proc 型・メソッド型の引数名を除く (`^(Integer x) -> void` → `^(Integer) -> void`)",
    "空白の違いを無視する (トークン単位で組み直す)",
    "Steep の `` **Local variable** `x: T` `` や ruby-lsp-rs の `x: T` のような変数名の接頭辞を除いて型だけを取り出す",
    "メソッドの戻り値は、Steep のメソッド型 (`def m: (...) -> R`) と ruby-lsp-rs の `(Receiver) → R` から R を取り出して比べる",
    "メソッド呼び出しで Steep の戻り値に残る `self` は、ruby-lsp-rs の hover が示すレシーバー型 (`(Receiver) → R` の Receiver) に置き換える",
    "Steep の論理型 (`<% Steep::AST::Types::Logic::ReceiverIsNil %>` など、`nil?` や `is_a?` の戻り値) を `bool` とみなす",
    "proc 型の self 束縛 (`[self: T]`) は残して比べる",
    "構文解析できない型は `::` の除去と空白の正規化だけを行う",
]

TOKEN_PATTERN = re.compile(
    r"""
    (?P<space>\s+)
    | (?P<arrow>->)
    | (?P<double_star>\*\*)
    | (?P<string>"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')
    | (?P<symbol>:(?:[A-Za-z_][\w]*[?!=]?|"(?:[^"\\]|\\.)*"))
    | (?P<integer>-?\d+)
    | (?P<name>(?:::)?[A-Za-z_][\w]*(?:::[A-Za-z_][\w]*)*[?!]?(?=\s*:(?!:)))
    | (?P<type_name>(?:::)?[A-Za-z_][\w]*(?:::[A-Za-z_][\w]*)*)
    | (?P<punctuation>[()\[\]{}|&?^,*:=>.])
    """,
    re.VERBOSE,
)


class TypeParseError(Exception):
    pass


def tokenize(text):
    tokens = []
    position = 0
    while position < len(text):
        match = TOKEN_PATTERN.match(text, position)
        if not match:
            raise TypeParseError(f"unexpected character {text[position]!r} in {text!r}")
        position = match.end()
        kind = match.lastgroup
        if kind == "space":
            continue
        tokens.append((kind, match.group(0)))
    return tokens


def strip_leading_colons(name):
    return re.sub(r"(?<![\w:])::", "", name)


class TypeParser:
    def __init__(self, text):
        self.tokens = tokenize(text)
        self.index = 0

    def peek(self, offset=0):
        if self.index + offset < len(self.tokens):
            return self.tokens[self.index + offset]
        return (None, None)

    def peek_value(self, offset=0):
        return self.peek(offset)[1]

    def advance(self):
        token = self.peek()
        if token[0] is None:
            raise TypeParseError("unexpected end of type")
        self.index += 1
        return token

    def expect(self, value):
        kind, actual = self.advance()
        if actual != value:
            raise TypeParseError(f"expected {value!r} but got {actual!r}")

    def at_end(self):
        return self.index >= len(self.tokens)

    def parse_type(self):
        members = [self.parse_intersection()]
        while self.peek_value() == "|":
            self.advance()
            members.append(self.parse_intersection())
        if len(members) == 1:
            return members[0]
        return ("union", members)

    def parse_intersection(self):
        members = [self.parse_postfix()]
        while self.peek_value() == "&":
            self.advance()
            members.append(self.parse_postfix())
        if len(members) == 1:
            return members[0]
        return ("intersection", members)

    def parse_postfix(self):
        node = self.parse_primary()
        while self.peek_value() == "?":
            self.advance()
            node = ("union", [node, ("atom", "nil")])
        return node

    def parse_primary(self):
        kind, value = self.peek()
        if value == "(":
            self.advance()
            inner = self.parse_type()
            self.expect(")")
            return inner
        if value == "^":
            self.advance()
            return ("proc", self.parse_callable())
        if value == "[":
            self.advance()
            elements = []
            while self.peek_value() != "]":
                elements.append(self.parse_type())
                if self.peek_value() == ",":
                    self.advance()
            self.expect("]")
            return ("tuple", elements)
        if value == "{":
            return self.parse_record()
        if kind in ("string", "symbol", "integer"):
            self.advance()
            return ("atom", value)
        if kind in ("type_name", "name"):
            self.advance()
            name = strip_leading_colons(value)
            if name == "singleton" and self.peek_value() == "(":
                self.advance()
                inner = self.parse_type()
                self.expect(")")
                return ("singleton", inner)
            if self.peek_value() == "[":
                self.advance()
                arguments = []
                while self.peek_value() != "]":
                    arguments.append(self.parse_type())
                    if self.peek_value() == ",":
                        self.advance()
                self.expect("]")
                return ("application", name, arguments)
            return ("atom", name)
        raise TypeParseError(f"unexpected token {value!r}")

    def parse_record(self):
        self.expect("{")
        fields = []
        while self.peek_value() != "}":
            optional = ""
            if self.peek_value() == "?":
                self.advance()
                optional = "?"
            kind, value = self.advance()
            if self.peek_value() == ":":
                self.advance()
                key = f"{optional}{value}:"
            elif self.peek_value() == "=":
                self.advance()
                self.expect(">")
                key = f"{optional}{value} =>"
            else:
                raise TypeParseError("unexpected record key")
            fields.append((key, self.parse_type()))
            if self.peek_value() == ",":
                self.advance()
        self.expect("}")
        return ("record", fields)

    def parse_parameters(self):
        parameters = []
        self.expect("(")
        while self.peek_value() != ")":
            prefix = ""
            if self.peek_value() == "?":
                self.advance()
                prefix = "?"
            if self.peek_value() in ("*", "**"):
                prefix += self.advance()[1]
            if self.peek()[0] == "name" and self.peek_value(1) == ":":
                prefix += self.advance()[1] + ": "
                self.advance()
            parameter_type = self.parse_type()
            if self.peek()[0] in ("type_name", "name") and self.peek_value() not in ("|", "&"):
                self.advance()
            parameters.append((prefix, parameter_type))
            if self.peek_value() == ",":
                self.advance()
        self.expect(")")
        return parameters

    def parse_self_binding(self):
        if self.peek_value() == "[" and self.peek_value(1) == "self":
            self.advance()
            self.advance()
            self.expect(":")
            self_type = self.parse_type()
            self.expect("]")
            return self_type
        return None

    def parse_callable(self):
        parameters = None
        if self.peek_value() == "(":
            parameters = self.parse_parameters()
        self_binding = self.parse_self_binding()
        block = None
        if self.peek_value() == "?" and self.peek_value(1) == "{":
            self.advance()
        if self.peek_value() == "{":
            self.advance()
            block_parameters = self.parse_parameters() if self.peek_value() == "(" else None
            self.parse_self_binding()
            self.expect("->")
            block = (block_parameters, self.parse_type())
            self.expect("}")
        self.expect("->")
        return_type = self.parse_postfix()
        return (parameters, block, return_type, self_binding)


def flatten(kind, members):
    flattened = []
    for member in members:
        if member[0] == kind:
            flattened.extend(flatten(kind, member[1]))
        else:
            flattened.append(member)
    return flattened


def render(node, inside_operator=False):
    kind = node[0]
    if kind == "atom":
        return node[1]
    if kind == "application":
        return f"{node[1]}[{', '.join(render(argument) for argument in node[2])}]"
    if kind == "singleton":
        return f"singleton({render(node[1])})"
    if kind == "tuple":
        return f"[{', '.join(render(element) for element in node[1])}]"
    if kind == "record":
        return "{ " + ", ".join(f"{key} {render(value)}" for key, value in node[1]) + " }"
    if kind == "proc":
        return "^" + render_callable(node[1])
    if kind == "intersection":
        members = sorted({render(member, inside_operator=True) for member in flatten("intersection", node[1])})
        text = " & ".join(members)
        return f"({text})" if inside_operator or len(members) > 1 else text
    if kind == "union":
        members = {render(member, inside_operator=True) for member in flatten("union", node[1])}
        has_nil = "nil" in members
        members.discard("nil")
        if "bool" in members:
            members.discard("bool")
            members.update({"true", "false"})
        if {"true", "false"} <= members:
            members -= {"true", "false"}
            members.add("bool")
        ordered = sorted(members)
        if not ordered:
            return "nil"
        if len(ordered) == 1:
            text = ordered[0]
            if has_nil:
                return text + "?"
            return text
        text = "(" + " | ".join(ordered) + ")"
        return text + "?" if has_nil else (text if inside_operator else text[1:-1])
    raise TypeParseError(f"unknown node {kind}")


def render_parameters(parameters):
    return "(" + ", ".join(prefix + render(parameter_type) for prefix, parameter_type in parameters) + ")"


def render_callable(callable_node):
    parameters, block, return_type, self_binding = callable_node
    text = render_parameters(parameters) + " " if parameters is not None else ""
    if self_binding is not None:
        text += f"[self: {render(self_binding)}] "
    if block:
        block_parameters, block_return = block
        block_text = render_parameters(block_parameters) + " " if block_parameters is not None else ""
        text += "{ " + block_text + "-> " + render(block_return) + " } "
    return text + "-> " + render(return_type, inside_operator=True)


def fallback_normalize(text):
    return re.sub(r"\s+", " ", strip_leading_colons(text)).strip()


def substitute_self(node, self_node):
    kind = node[0]
    if kind == "atom":
        return self_node if node[1] == "self" else node
    if kind == "application":
        return ("application", node[1], [substitute_self(argument, self_node) for argument in node[2]])
    if kind in ("union", "intersection", "tuple"):
        return (kind, [substitute_self(member, self_node) for member in node[1]])
    if kind == "singleton":
        return ("singleton", substitute_self(node[1], self_node))
    return node


STEEP_LOGIC_TYPE = re.compile(r"<%\s*Steep::AST::Types::Logic::\w+\s*%>")


def normalize_type(text, self_type=None):
    if text is None:
        return None
    text = STEEP_LOGIC_TYPE.sub("bool", text.strip())
    try:
        parser = TypeParser(text)
        node = parser.parse_type()
        if self_type:
            self_parser = TypeParser(self_type)
            node = substitute_self(node, self_parser.parse_type())
        if not parser.at_end():
            raise TypeParseError("trailing tokens")
        return render(node)
    except (TypeParseError, RecursionError):
        return fallback_normalize(text)


def split_method_type_overloads(text):
    overloads = []
    depth = 0
    current = []
    for line in text.split("\n"):
        stripped = line.strip()
        if stripped.startswith("| ") and depth == 0 and current:
            overloads.append(" ".join(current))
            current = [stripped[2:]]
        else:
            current.append(stripped)
        depth += sum(line.count(character) for character in "([{") - sum(line.count(character) for character in ")]}")
    if current:
        overloads.append(" ".join(current))
    return overloads


def method_type_return(text):
    text = text.strip()
    try:
        parser = TypeParser(text)
        if parser.peek_value() == "[":
            depth = 0
            while True:
                value = parser.advance()[1]
                if value == "[":
                    depth += 1
                elif value == "]":
                    depth -= 1
                    if depth == 0:
                        break
        _, _, return_type, _ = parser.parse_callable()
        if not parser.at_end():
            raise TypeParseError("trailing tokens")
        return return_type
    except TypeParseError:
        return None


def method_types_return(method_types_text):
    returns = [method_type_return(overload) for overload in split_method_type_overloads(method_types_text)]
    if not returns or any(return_type is None for return_type in returns):
        return None
    if len(returns) == 1:
        return render(returns[0])
    return render(("union", returns))


BACKTICKED = re.compile(r"`([^`]*)`")


def normalize_message(message):
    message = BACKTICKED.sub(lambda match: "`" + normalize_type(match.group(1)) + "`", message)
    return fallback_normalize(message)
