use std::{ffi::CStr, os::raw::c_char, sync::Arc};

use ruby_rbs::node::{
    self, AliasKind, AttrIvarName, AttributeVisibility, MethodDefinitionKind,
    MethodDefinitionVisibility, NamespaceNode, Node, NodeList, RBSLocationRange, SymbolNode,
    TypeNameNode, TypeParamVariance,
};
use ruby_rbs_sys::bindings::{
    RBSTokenType, RBSTokenType_pEOF, rbs_encoding_t, rbs_encoding_type_t::RBS_ENCODING_UTF_8,
    rbs_encodings, rbs_method_type_t, rbs_node_t, rbs_parse_signature, rbs_parser_free,
    rbs_parser_new, rbs_parser_t, rbs_signature_t, rbs_string_new,
};

use crate::{
    ast::{
        Alias, AliasDeclaration, Attribute, AttributeInstanceVariable, AttributeKind, Block,
        Callable, ClassDeclaration, ClassOrModule, ConstantDeclaration, Declaration, Directive,
        Function, GlobalDeclaration, InterfaceDeclaration, Literal, Member, MemberKind,
        MethodDefinition, MethodKind, MethodType, Mixin, MixinKind, ModuleDeclaration, ModuleSelf,
        Namespace, Overload, Parameter, RecordField, Super, Type, TypeAliasDeclaration, TypeName,
        TypeParameter, UseClause, Variable, VariableKind, Variance, Visibility,
    },
    inline_parser::annotation,
    location::{AstLocation, Buffer, Location, TypeLocation},
};

#[derive(Clone, Debug)]
pub struct ParsingError {
    pub location: Location,
    pub error_message: String,
    pub token_type: String,
}

impl ParsingError {
    pub fn message(&self) -> String {
        format!(
            "{}: Syntax error: {}, token=`{}` ({})",
            self.location,
            self.error_message,
            self.location.source(),
            self.token_type
        )
    }
}

#[derive(Clone, Debug)]
pub enum SignatureError {
    Syntax(ParsingError),
    Unsupported(String),
}

impl std::fmt::Display for SignatureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignatureError::Syntax(error) => formatter.write_str(&error.message()),
            SignatureError::Unsupported(message) => formatter.write_str(message),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Signature {
    pub directives: Vec<Directive>,
    pub declarations: Vec<Declaration>,
}

pub fn parse_signature(source: &str) -> Result<Signature, String> {
    parse_signature_buffer(&Buffer::new("", source)).map_err(|error| error.to_string())
}

pub fn parse_signature_buffer(buffer: &Arc<Buffer>) -> Result<Signature, SignatureError> {
    let source = buffer.content.as_str();
    let signature = match node::parse(source) {
        Ok(signature) => signature,
        Err(message) => {
            return Err(match syntax_error(buffer) {
                Some(error) => SignatureError::Syntax(error),
                None => SignatureError::Unsupported(message),
            });
        }
    };
    let parser = Parser {
        buffer,
        source,
        position: &|position| position,
    };
    Ok(Signature {
        directives: signature.directives().iter().map(directive).collect(),
        declarations: signature
            .declarations()
            .iter()
            .map(|node| parser.declaration(node))
            .collect(),
    })
}

fn directive(node: Node) -> Directive {
    let Node::Use(use_node) = node else {
        unreachable!("a signature has only use directives")
    };
    Directive::Use(
        use_node
            .clauses()
            .iter()
            .map(|clause| match clause {
                Node::UseSingleClause(clause) => UseClause::Single {
                    type_name: type_name(clause.type_name()),
                    new_name: clause.new_name().map(symbol),
                },
                Node::UseWildcardClause(clause) => UseClause::Wildcard {
                    namespace: namespace(clause.namespace()),
                },
                _ => unreachable!("a use directive has only use clauses"),
            })
            .collect(),
    )
}

/// Converts a type node parsed from a part of `buffer`, whose positions are `shift` bytes away from the positions in `buffer`.
pub fn located_type(node: Node, buffer: &Arc<Buffer>, shift: isize) -> Type {
    Parser {
        buffer,
        source: &buffer.content,
        position: &|position| (position as isize + shift).max(0) as usize,
    }
    .type_(node)
}

pub fn type_(node: Node) -> Type {
    located_type(node, &Buffer::new("", ""), 0)
}

unsafe extern "C" {
    fn rbs_token_type_str(token_type: RBSTokenType) -> *const c_char;
    fn rbs_parse_type(
        parser: *mut rbs_parser_t,
        type_: *mut *mut rbs_node_t,
        void_allowed: bool,
        self_allowed: bool,
        classish_allowed: bool,
    ) -> bool;
    fn rbs_parse_method_type(
        parser: *mut rbs_parser_t,
        method_type: *mut *mut rbs_method_type_t,
        require_eof: bool,
        classish_allowed: bool,
    ) -> bool;
}

fn parsing_error(parser: &rbs_parser_t, buffer: &Arc<Buffer>) -> Option<ParsingError> {
    let error = unsafe { parser.error.as_ref()? };
    if !error.syntax_error || error.message.is_null() {
        return None;
    }
    unsafe {
        Some(ParsingError {
            location: Location::new(
                buffer,
                error.token.range.start.byte_pos.max(0) as usize,
                error.token.range.end.byte_pos.max(0) as usize,
            ),
            error_message: CStr::from_ptr(error.message).to_string_lossy().into_owned(),
            token_type: CStr::from_ptr(rbs_token_type_str(error.token.type_))
                .to_string_lossy()
                .into_owned(),
        })
    }
}

fn syntax_error(buffer: &Arc<Buffer>) -> Option<ParsingError> {
    let source = buffer.content.as_str();
    unsafe {
        let start = source.as_ptr().cast::<c_char>();
        let string = rbs_string_new(start, start.add(source.len()));
        let encoding = &rbs_encodings[RBS_ENCODING_UTF_8 as usize] as *const rbs_encoding_t;
        let parser = rbs_parser_new(string, encoding, 0, source.len() as i32);
        let mut signature: *mut rbs_signature_t = std::ptr::null_mut();
        let succeeded = rbs_parse_signature(parser, &mut signature);
        let error = if succeeded {
            None
        } else {
            parsing_error(&*parser, buffer)
        };
        rbs_parser_free(parser);
        error
    }
}

#[derive(Clone, Debug)]
pub struct ParsedType {
    pub type_: Type,
    pub location: Location,
}

pub fn parse_whole_type_node<R>(
    source: &str,
    convert: impl FnOnce(Node) -> R,
) -> Result<Option<R>, ParsingError> {
    let buffer = Buffer::new("", source);
    let parser = annotation::Parser::new(source, 0, source.len());
    if parser.raw().next_token.type_ == RBSTokenType_pEOF {
        return Ok(None);
    }
    let mut type_: *mut rbs_node_t = std::ptr::null_mut();
    let succeeded = unsafe { rbs_parse_type(parser.pointer(), &mut type_, true, true, true) };
    if let Some(error) = parsing_error(parser.raw(), &buffer) {
        return Err(error);
    }
    if !succeeded || type_.is_null() {
        return Ok(None);
    }
    let next_token = parser.raw().next_token;
    if next_token.type_ != RBSTokenType_pEOF {
        let token_type = unsafe { CStr::from_ptr(rbs_token_type_str(next_token.type_)) }
            .to_string_lossy()
            .into_owned();
        return Err(ParsingError {
            location: Location::new(
                &buffer,
                next_token.range.start.byte_pos.max(0) as usize,
                next_token.range.end.byte_pos.max(0) as usize,
            ),
            error_message: "expected a token `pEOF`".to_string(),
            token_type,
        });
    }
    Ok(Some(convert(parser.node(type_))))
}

pub fn parse_type(source: &str) -> Result<Option<ParsedType>, ParsingError> {
    let buffer = Buffer::new("", source);
    let parser = annotation::Parser::new(source, 0, source.len());
    if parser.raw().next_token.type_ == RBSTokenType_pEOF {
        return Ok(None);
    }
    let mut type_: *mut rbs_node_t = std::ptr::null_mut();
    let succeeded = unsafe { rbs_parse_type(parser.pointer(), &mut type_, true, true, true) };
    if let Some(error) = parsing_error(parser.raw(), &buffer) {
        return Err(error);
    }
    if !succeeded || type_.is_null() {
        return Ok(None);
    }
    let node = parser.node(type_);
    let range = type_location(&node).expect("a parsed type has a location");
    let located = Parser {
        buffer: &buffer,
        source,
        position: &|position| position,
    };
    Ok(Some(ParsedType {
        location: Location::new(&buffer, range.start() as usize, range.end() as usize),
        type_: located.type_(node),
    }))
}

pub fn parse_method_type(source: &str) -> Result<Option<MethodType>, ParsingError> {
    let buffer = Buffer::new("", source);
    let parser = annotation::Parser::new(source, 0, source.len());
    if parser.raw().next_token.type_ == RBSTokenType_pEOF {
        return Ok(None);
    }
    let mut method_type: *mut rbs_method_type_t = std::ptr::null_mut();
    let succeeded =
        unsafe { rbs_parse_method_type(parser.pointer(), &mut method_type, false, true) };
    if let Some(error) = parsing_error(parser.raw(), &buffer) {
        return Err(error);
    }
    if !succeeded || method_type.is_null() {
        return Ok(None);
    }
    let located = Parser {
        buffer: &buffer,
        source,
        position: &|position| position,
    };
    Ok(Some(located.method_type(
        parser.node(method_type.cast::<rbs_node_t>()),
    )))
}

fn substitute_variable(variables: &[String], type_: &Type) -> Type {
    if let Type::ClassInstance { name, .. } = type_
        && !name.namespace.absolute
        && name.namespace.is_empty()
        && variables.contains(&name.name)
    {
        return Type::Variable(name.name.clone());
    }
    type_.map_type(&mut |child| substitute_variable(variables, child))
}

fn resolve_variables(parameters: Vec<TypeParameter>) -> Vec<TypeParameter> {
    let variables: Vec<String> = parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect();
    let substitute =
        |type_: Option<Type>| type_.map(|type_| substitute_variable(&variables, &type_));
    parameters
        .into_iter()
        .map(|parameter| TypeParameter {
            upper_bound: substitute(parameter.upper_bound.clone()),
            lower_bound: substitute(parameter.lower_bound.clone()),
            default_type: substitute(parameter.default_type.clone()),
            ..parameter
        })
        .collect()
}

pub(crate) struct Parser<'buffer> {
    pub(crate) buffer: &'buffer Arc<Buffer>,
    pub(crate) source: &'buffer str,
    pub(crate) position: &'buffer dyn Fn(usize) -> usize,
}

pub(crate) fn symbol(node: SymbolNode) -> String {
    String::from_utf8_lossy(node.as_bytes()).into_owned()
}

fn namespace(node: NamespaceNode) -> Namespace {
    Namespace {
        absolute: node.absolute(),
        path: node
            .path()
            .iter()
            .map(|segment| match segment {
                Node::Symbol(segment) => symbol(segment),
                _ => unreachable!("a namespace path has only symbols"),
            })
            .collect(),
    }
}

pub(crate) fn type_name(node: TypeNameNode) -> TypeName {
    TypeName {
        namespace: namespace(node.namespace()),
        name: symbol(node.name()),
    }
}

pub(crate) fn annotations(list: NodeList) -> Vec<String> {
    list.iter()
        .map(|annotation| match annotation {
            Node::Annotation(annotation) => annotation.string().as_str().to_string(),
            _ => unreachable!("an annotation list has only annotations"),
        })
        .collect()
}

fn literal(node: Node) -> Literal {
    match node {
        Node::Integer(integer) => {
            Literal::Integer(integer.string_representation().as_str().to_string())
        }
        Node::String(string) => Literal::String(string.string().as_str().to_string()),
        Node::Symbol(symbol_node) => Literal::Symbol(symbol(symbol_node)),
        Node::Bool(boolean) => Literal::Bool(boolean.value()),
        _ => unreachable!("not a literal"),
    }
}

fn method_visibility(visibility: MethodDefinitionVisibility) -> Option<Visibility> {
    match visibility {
        MethodDefinitionVisibility::Unspecified => None,
        MethodDefinitionVisibility::Public => Some(Visibility::Public),
        MethodDefinitionVisibility::Private => Some(Visibility::Private),
    }
}

fn attribute_visibility(visibility: AttributeVisibility) -> Option<Visibility> {
    match visibility {
        AttributeVisibility::Unspecified => None,
        AttributeVisibility::Public => Some(Visibility::Public),
        AttributeVisibility::Private => Some(Visibility::Private),
    }
}

fn attribute_kind(kind: node::AttributeKind) -> MemberKind {
    match kind {
        node::AttributeKind::Instance => MemberKind::Instance,
        node::AttributeKind::Singleton => MemberKind::Singleton,
    }
}

fn type_location(node: &Node) -> Option<RBSLocationRange> {
    Some(match node {
        Node::BoolType(type_) => type_.location(),
        Node::VoidType(type_) => type_.location(),
        Node::AnyType(type_) => type_.location(),
        Node::NilType(type_) => type_.location(),
        Node::TopType(type_) => type_.location(),
        Node::BottomType(type_) => type_.location(),
        Node::SelfType(type_) => type_.location(),
        Node::InstanceType(type_) => type_.location(),
        Node::ClassType(type_) => type_.location(),
        Node::VariableType(type_) => type_.location(),
        Node::ClassInstanceType(type_) => type_.location(),
        Node::ClassSingletonType(type_) => type_.location(),
        Node::InterfaceType(type_) => type_.location(),
        Node::AliasType(type_) => type_.location(),
        Node::TupleType(type_) => type_.location(),
        Node::RecordType(type_) => type_.location(),
        Node::OptionalType(type_) => type_.location(),
        Node::UnionType(type_) => type_.location(),
        Node::IntersectionType(type_) => type_.location(),
        Node::ProcType(type_) => type_.location(),
        Node::LiteralType(type_) => type_.location(),
        _ => return None,
    })
}

impl Parser<'_> {
    pub(crate) fn location(&self, range: RBSLocationRange) -> Option<Location> {
        if range.start() < 0 || range.end() < 0 {
            return None;
        }
        Some(Location::new(
            self.buffer,
            (self.position)(range.start() as usize),
            (self.position)(range.end() as usize),
        ))
    }

    pub(crate) fn optional_location(&self, range: Option<RBSLocationRange>) -> Option<Location> {
        range.and_then(|range| self.location(range))
    }

    pub(crate) fn type_node_location(&self, node: &Node) -> Option<Location> {
        self.optional_location(type_location(node))
    }

    fn named_type_location(
        &self,
        location: RBSLocationRange,
        name_location: RBSLocationRange,
    ) -> TypeLocation {
        TypeLocation::new(self.location(location), self.location(name_location))
    }

    pub(crate) fn types(&self, list: NodeList) -> Vec<Type> {
        list.iter().map(|node| self.type_(node)).collect()
    }

    pub(crate) fn type_(&self, node: Node) -> Type {
        match node {
            Node::BoolType(_) => Type::Bool,
            Node::VoidType(_) => Type::Void,
            Node::AnyType(_) => Type::Any,
            Node::NilType(_) => Type::Nil,
            Node::TopType(_) => Type::Top,
            Node::BottomType(_) => Type::Bottom,
            Node::SelfType(_) => Type::SelfType,
            Node::InstanceType(_) => Type::Instance,
            Node::ClassType(_) => Type::Class,
            Node::VariableType(variable) => Type::Variable(symbol(variable.name())),
            Node::ClassInstanceType(instance) => Type::ClassInstance {
                name: type_name(instance.name()),
                args: self.types(instance.args()),
                location: self.named_type_location(instance.location(), instance.name_location()),
            },
            Node::ClassSingletonType(singleton) => Type::ClassSingleton {
                name: type_name(singleton.name()),
                args: self.types(singleton.args()),
                location: self.named_type_location(singleton.location(), singleton.name_location()),
            },
            Node::InterfaceType(interface) => Type::Interface {
                name: type_name(interface.name()),
                args: self.types(interface.args()),
                location: self.named_type_location(interface.location(), interface.name_location()),
            },
            Node::AliasType(alias) => Type::Alias {
                name: type_name(alias.name()),
                args: self.types(alias.args()),
                location: self.named_type_location(alias.location(), alias.name_location()),
            },
            Node::TupleType(tuple) => Type::Tuple(self.types(tuple.types())),
            Node::RecordType(record) => Type::Record(
                record
                    .all_fields()
                    .iter()
                    .map(|(key, value)| match value {
                        Node::RecordFieldType(field) => RecordField {
                            key: literal(key),
                            type_: self.type_(field.type_()),
                            required: field.required(),
                        },
                        _ => unreachable!("a record has only fields"),
                    })
                    .collect(),
            ),
            Node::OptionalType(optional) => Type::Optional(Box::new(self.type_(optional.type_()))),
            Node::UnionType(union) => Type::Union(self.types(union.types())),
            Node::IntersectionType(intersection) => {
                Type::Intersection(self.types(intersection.types()))
            }
            Node::ProcType(proc_type) => Type::Proc {
                function: Box::new(self.callable(proc_type.type_())),
                block: proc_type
                    .block()
                    .map(|block_node| Box::new(self.block(block_node))),
                self_type: proc_type
                    .self_type()
                    .map(|self_type| Box::new(self.type_(self_type))),
            },
            Node::LiteralType(literal_type) => Type::Literal(literal(literal_type.literal())),
            _ => unreachable!("not a type"),
        }
    }

    fn parameter(&self, node: Node) -> Parameter {
        match node {
            Node::FunctionParam(parameter) => Parameter {
                type_: self.type_(parameter.type_()),
                name: parameter.name().map(symbol),
            },
            _ => unreachable!("not a function parameter"),
        }
    }

    fn parameters(&self, list: NodeList) -> Vec<Parameter> {
        list.iter().map(|node| self.parameter(node)).collect()
    }

    fn keywords(&self, hash: node::RBSHash) -> Vec<(String, Parameter)> {
        hash.iter()
            .map(|(key, value)| match key {
                Node::Symbol(key) => (symbol(key), self.parameter(value)),
                _ => unreachable!("a keyword is a symbol"),
            })
            .collect()
    }

    pub(crate) fn callable(&self, node: Node) -> Callable {
        match node {
            Node::FunctionType(function) => Callable::Function(Box::new(Function {
                required_positionals: self.parameters(function.required_positionals()),
                optional_positionals: self.parameters(function.optional_positionals()),
                rest_positionals: function.rest_positionals().map(|node| self.parameter(node)),
                trailing_positionals: self.parameters(function.trailing_positionals()),
                required_keywords: self.keywords(function.required_keywords()),
                optional_keywords: self.keywords(function.optional_keywords()),
                rest_keywords: function.rest_keywords().map(|node| self.parameter(node)),
                return_type: self.type_(function.return_type()),
            })),
            Node::UntypedFunctionType(function) => Callable::Untyped {
                return_type: self.type_(function.return_type()),
            },
            _ => unreachable!("not a function"),
        }
    }

    pub(crate) fn block(&self, node: node::BlockTypeNode) -> Block {
        Block {
            type_: self.callable(node.type_()),
            required: node.required(),
            self_type: node.self_type().map(|node| self.type_(node)),
        }
    }

    fn type_parameters(&self, list: NodeList) -> Vec<TypeParameter> {
        resolve_variables(self.parsed_type_parameters(list))
    }

    fn parsed_type_parameters(&self, list: NodeList) -> Vec<TypeParameter> {
        list.iter()
            .map(|parameter| match parameter {
                Node::TypeParam(parameter) => TypeParameter {
                    name: symbol(parameter.name()),
                    variance: match parameter.variance() {
                        TypeParamVariance::Invariant => Variance::Invariant,
                        TypeParamVariance::Covariant => Variance::Covariant,
                        TypeParamVariance::Contravariant => Variance::Contravariant,
                    },
                    unchecked: parameter.unchecked(),
                    upper_bound: parameter.upper_bound().map(|node| self.type_(node)),
                    lower_bound: parameter.lower_bound().map(|node| self.type_(node)),
                    default_type_location: AstLocation::new(
                        parameter
                            .default_type()
                            .and_then(|node| self.type_node_location(&node)),
                    ),
                    default_type: parameter.default_type().map(|node| self.type_(node)),
                    location: AstLocation::new(self.location(parameter.location())),
                },
                _ => unreachable!("not a type parameter"),
            })
            .collect()
    }

    pub(crate) fn method_type(&self, node: Node) -> MethodType {
        match node {
            Node::MethodType(method_type) => MethodType {
                type_parameters: self.type_parameters(method_type.type_params()),
                type_: self.callable(method_type.type_()),
                block: method_type.block().map(|node| self.block(node)),
                location: AstLocation::new(self.location(method_type.location())),
            },
            _ => unreachable!("not a method type"),
        }
    }

    fn attribute_instance_variable(
        &self,
        instance_variable: AttrIvarName,
        location: Option<RBSLocationRange>,
    ) -> AttributeInstanceVariable {
        match instance_variable {
            AttrIvarName::Unspecified => AttributeInstanceVariable::Unspecified,
            AttrIvarName::Empty => AttributeInstanceVariable::Empty,
            AttrIvarName::Name(_) => {
                let location = location.expect("a named instance variable has a location");
                AttributeInstanceVariable::Name(
                    self.source[location.start() as usize..location.end() as usize].to_string(),
                )
            }
        }
    }

    fn members(&self, list: NodeList) -> Vec<Member> {
        list.iter().map(|node| self.member(node)).collect()
    }

    fn mixin(
        &self,
        mixin_kind: MixinKind,
        name: TypeNameNode,
        args: NodeList,
        location: RBSLocationRange,
    ) -> Member {
        Member::Mixin(Mixin {
            mixin_kind,
            name: type_name(name),
            args: self.types(args),
            location: self.location(location),
        })
    }

    fn variable(
        &self,
        variable_kind: VariableKind,
        name: SymbolNode,
        type_node: Node,
        location: RBSLocationRange,
        name_location: RBSLocationRange,
    ) -> Member {
        Member::Variable(Variable {
            variable_kind,
            name: symbol(name),
            type_location: self.type_node_location(&type_node),
            type_: self.type_(type_node),
            location: self.location(location),
            name_location: self.location(name_location),
        })
    }

    fn member(&self, node: Node) -> Member {
        macro_rules! attribute {
            ($attribute:expr, $kind:expr) => {{
                let attribute = $attribute;
                let type_node = attribute.type_();
                Member::Attribute(Attribute {
                    attribute_kind: $kind,
                    name: symbol(attribute.name()),
                    type_location: self.type_node_location(&type_node),
                    type_: self.type_(type_node),
                    kind: attribute_kind(attribute.kind()),
                    instance_variable: self.attribute_instance_variable(
                        attribute.ivar_name(),
                        attribute.ivar_name_location(),
                    ),
                    visibility: attribute_visibility(attribute.visibility()),
                    annotations: annotations(attribute.annotations()),
                    location: self.location(attribute.location()),
                    declared_in_ruby: false,
                })
            }};
        }
        match node {
            Node::MethodDefinition(definition) => Member::MethodDefinition(MethodDefinition {
                name: symbol(definition.name()),
                kind: match definition.kind() {
                    MethodDefinitionKind::Instance => MethodKind::Instance,
                    MethodDefinitionKind::Singleton => MethodKind::Singleton,
                    MethodDefinitionKind::SingletonInstance => MethodKind::SingletonInstance,
                },
                overloads: definition
                    .overloads()
                    .iter()
                    .map(|overload| match overload {
                        Node::MethodDefinitionOverload(overload) => Overload {
                            method_type: Arc::new(self.method_type(overload.method_type())),
                            annotations: annotations(overload.annotations()),
                        },
                        _ => unreachable!("not an overload"),
                    })
                    .collect(),
                overloading: definition.overloading(),
                visibility: method_visibility(definition.visibility()),
                annotations: annotations(definition.annotations()),
                location: self.location(definition.location()),
                declared_in_ruby: false,
                unannotated_ruby_definition: false,
            }),
            Node::AttrReader(attribute) => attribute!(attribute, AttributeKind::Reader),
            Node::AttrWriter(attribute) => attribute!(attribute, AttributeKind::Writer),
            Node::AttrAccessor(attribute) => attribute!(attribute, AttributeKind::Accessor),
            Node::Alias(alias) => Member::Alias(Alias {
                new_name: symbol(alias.new_name()),
                old_name: symbol(alias.old_name()),
                kind: match alias.kind() {
                    AliasKind::Instance => MemberKind::Instance,
                    AliasKind::Singleton => MemberKind::Singleton,
                },
                annotations: annotations(alias.annotations()),
                location: self.location(alias.location()),
            }),
            Node::Include(mixin) => self.mixin(
                MixinKind::Include,
                mixin.name(),
                mixin.args(),
                mixin.location(),
            ),
            Node::Extend(mixin) => self.mixin(
                MixinKind::Extend,
                mixin.name(),
                mixin.args(),
                mixin.location(),
            ),
            Node::Prepend(mixin) => self.mixin(
                MixinKind::Prepend,
                mixin.name(),
                mixin.args(),
                mixin.location(),
            ),
            Node::InstanceVariable(variable) => self.variable(
                VariableKind::InstanceVariable,
                variable.name(),
                variable.type_(),
                variable.location(),
                variable.name_location(),
            ),
            Node::ClassInstanceVariable(variable) => self.variable(
                VariableKind::ClassInstanceVariable,
                variable.name(),
                variable.type_(),
                variable.location(),
                variable.name_location(),
            ),
            Node::ClassVariable(variable) => self.variable(
                VariableKind::ClassVariable,
                variable.name(),
                variable.type_(),
                variable.location(),
                variable.name_location(),
            ),
            Node::Public(_) => Member::Visibility(Visibility::Public),
            Node::Private(_) => Member::Visibility(Visibility::Private),
            node => Member::Declaration(self.declaration(node)),
        }
    }

    fn declaration(&self, node: Node) -> Declaration {
        match node {
            Node::Class(class) => Declaration::Class(Arc::new(ClassDeclaration {
                name: type_name(class.name()),
                type_parameters: self.type_parameters(class.type_params()),
                super_class: class.super_class().map(|super_class| Super {
                    name: type_name(super_class.name()),
                    args: self.types(super_class.args()),
                    location: self.location(super_class.location()),
                }),
                members: self.members(class.members()),
                annotations: annotations(class.annotations()),
                location: self.location(class.location()),
                name_location: self.location(class.name_location()),
                declared_in_ruby: false,
            })),
            Node::Module(module) => Declaration::Module(Arc::new(ModuleDeclaration {
                name: type_name(module.name()),
                type_parameters: self.type_parameters(module.type_params()),
                self_types: module
                    .self_types()
                    .iter()
                    .map(|self_type| match self_type {
                        Node::ModuleSelf(self_type) => ModuleSelf {
                            name: type_name(self_type.name()),
                            args: self.types(self_type.args()),
                            location: self.location(self_type.location()),
                        },
                        _ => unreachable!("not a module self type"),
                    })
                    .collect(),
                members: self.members(module.members()),
                annotations: annotations(module.annotations()),
                location: self.location(module.location()),
                name_location: self.location(module.name_location()),
                declared_in_ruby: false,
            })),
            Node::Interface(interface) => Declaration::Interface(Arc::new(InterfaceDeclaration {
                name: type_name(interface.name()),
                type_parameters: self.type_parameters(interface.type_params()),
                members: self.members(interface.members()),
                annotations: annotations(interface.annotations()),
                location: self.location(interface.location()),
                name_location: self.location(interface.name_location()),
            })),
            Node::TypeAlias(alias) => {
                let type_node = alias.type_();
                Declaration::TypeAlias(Arc::new(TypeAliasDeclaration {
                    name: type_name(alias.name()),
                    type_parameters: self.type_parameters(alias.type_params()),
                    type_location: self.type_node_location(&type_node),
                    type_: self.type_(type_node),
                    annotations: annotations(alias.annotations()),
                    location: self.location(alias.location()),
                    name_location: self.location(alias.name_location()),
                    type_parameters_location: self.optional_location(alias.type_params_location()),
                }))
            }
            Node::Constant(constant) => Declaration::Constant(Arc::new(ConstantDeclaration {
                name: type_name(constant.name()),
                type_: self.type_(constant.type_()),
                annotations: annotations(constant.annotations()),
                location: self.location(constant.location()),
            })),
            Node::Global(global) => Declaration::Global(Arc::new(GlobalDeclaration {
                name: symbol(global.name()),
                type_: self.type_(global.type_()),
                annotations: annotations(global.annotations()),
                location: self.location(global.location()),
            })),
            Node::ClassAlias(alias) => Declaration::ClassAlias(Arc::new(AliasDeclaration {
                kind: ClassOrModule::Class,
                new_name: type_name(alias.new_name()),
                old_name: type_name(alias.old_name()),
                annotations: annotations(alias.annotations()),
                location: self.location(alias.location()),
                new_name_location: self.location(alias.new_name_location()),
                old_name_location: self.location(alias.old_name_location()),
            })),
            Node::ModuleAlias(alias) => Declaration::ClassAlias(Arc::new(AliasDeclaration {
                kind: ClassOrModule::Module,
                new_name: type_name(alias.new_name()),
                old_name: type_name(alias.old_name()),
                annotations: annotations(alias.annotations()),
                location: self.location(alias.location()),
                new_name_location: self.location(alias.new_name_location()),
                old_name_location: self.location(alias.old_name_location()),
            })),
            _ => unreachable!("not a declaration"),
        }
    }
}
