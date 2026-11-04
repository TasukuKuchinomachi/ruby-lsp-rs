pub mod annotation;
pub mod comment_block;
pub mod method_type;

use std::sync::Arc;

use ruby_prism::{
    CallNode, ClassNode, ConstantPathWriteNode, ConstantWriteNode, DefNode,
    Location as PrismLocation, ModuleNode, Node, Visit,
};

use crate::{
    ast::{
        AliasDeclaration, Attribute, AttributeInstanceVariable, AttributeKind, ClassDeclaration,
        ClassOrModule, ConstantDeclaration, Declaration, Member, MemberKind, MethodDefinition,
        MethodKind, Mixin, MixinKind, ModuleDeclaration, ModuleSelf, Namespace, Super, Type,
        TypeName, Variable, VariableKind, Visibility,
    },
    inline_parser::{
        annotation::{Annotation, ByteRange},
        comment_block::{
            Comment, CommentAssociation, CommentBlock, LocatedAnnotation, Paragraph, SourceLines,
            TrailingAnnotation,
        },
        method_type::{DefinitionParameter, MethodTypeAnnotation, Unused},
    },
    location::{Buffer, Location, TypeLocation},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    NotImplementedYet,
    NonConstantClassName,
    NonConstantModuleName,
    NonConstantSuperClassName,
    TopLevelMethodDefinition,
    TopLevelAttributeDefinition,
    NonConstantConstantDeclaration,
    UnusedInlineAnnotation,
    AnnotationSyntaxError,
    MixinMultipleArguments,
    MixinNonConstantModule,
    AttributeNonSymbolName,
    ClassModuleAliasDeclarationMissingTypeName,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub location: ByteRange,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct ParseResult {
    pub declarations: Vec<Declaration>,
    pub diagnostics: Vec<Diagnostic>,
    pub type_regions: Vec<TypeRegion>,
}

#[derive(Clone, Debug)]
pub struct TypeRegion {
    pub annotation: ByteRange,
    pub type_locations: Vec<Location>,
    pub type_name: Option<(TypeName, Location)>,
    pub context: Vec<TypeName>,
}

enum ModuleKind {
    Class { super_class: Option<Super> },
    Module,
}

struct PendingModule {
    kind: ModuleKind,
    name: TypeName,
    location: Option<Location>,
    name_location: Option<Location>,
    start_line: usize,
    members: Vec<(usize, PendingMember)>,
}

enum PendingMember {
    Member(Member),
    ModuleSelf(ModuleSelf),
    Module(PendingModule),
}

impl PendingModule {
    fn into_declaration(mut self) -> Declaration {
        self.members.sort_by_key(|(line, _)| *line);
        let mut self_types = Vec::new();
        let mut members = Vec::new();
        for (_, member) in self.members {
            match member {
                PendingMember::Member(member) => members.push(member),
                PendingMember::ModuleSelf(self_type) => self_types.push(self_type),
                PendingMember::Module(module) => {
                    members.push(Member::Declaration(module.into_declaration()))
                }
            }
        }
        match self.kind {
            ModuleKind::Class { super_class } => Declaration::Class(Arc::new(ClassDeclaration {
                name: self.name,
                type_parameters: Vec::new(),
                super_class,
                members,
                annotations: Vec::new(),
                location: self.location,
                name_location: self.name_location,
                comment: None,
                declared_in_ruby: true,
            })),
            ModuleKind::Module => Declaration::Module(Arc::new(ModuleDeclaration {
                name: self.name,
                type_parameters: Vec::new(),
                self_types,
                members,
                annotations: Vec::new(),
                location: self.location,
                name_location: self.name_location,
                comment: None,
                declared_in_ruby: true,
            })),
        }
    }
}

struct Parser<'source> {
    buffer: &'source Arc<Buffer>,
    source: &'source str,
    lines: SourceLines,
    comments: CommentAssociation,
    module_nesting: Vec<PendingModule>,
    result: ParseResult,
}

pub fn parse(buffer: &Arc<Buffer>) -> ParseResult {
    let source = buffer.content.as_str();
    let prism = ruby_prism::parse(source.as_bytes());
    let lines = SourceLines::new(source);
    let comments: Vec<Comment> = prism
        .comments()
        .filter(|comment| comment.type_() == ruby_prism::CommentType::InlineComment)
        .map(|comment| Comment {
            start: comment.location().start_offset(),
            end: comment.location().end_offset(),
        })
        .collect();
    let blocks = CommentBlock::build(buffer, &lines, &comments);
    let mut parser = Parser {
        buffer,
        source,
        lines,
        comments: CommentAssociation::new(blocks),
        module_nesting: Vec::new(),
        result: ParseResult::default(),
    };
    parser.visit(&prism.node());
    for index in parser.comments.unassociated_blocks() {
        parser.report_unused_block(index);
    }
    parser.result
}

impl Parser<'_> {
    fn type_context(&self, current: Option<&TypeName>) -> Vec<TypeName> {
        let mut context = Vec::new();
        let mut prefix = Namespace::root();
        for name in self
            .module_nesting
            .iter()
            .map(|module| &module.name)
            .chain(current)
        {
            context.push(name.with_prefix(&prefix));
            prefix = prefix.join(&name.to_namespace());
        }
        context
    }

    fn add_type_region(
        &mut self,
        annotation: ByteRange,
        type_locations: Vec<Option<Location>>,
        type_name: Option<(TypeName, Location)>,
        context: Vec<TypeName>,
    ) {
        self.result.type_regions.push(TypeRegion {
            annotation,
            type_locations: type_locations.into_iter().flatten().collect(),
            type_name,
            context,
        });
    }
}

fn range(location: &PrismLocation) -> ByteRange {
    ByteRange {
        start: location.start_offset(),
        end: location.end_offset(),
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn constant_path_name(node: &Node) -> Option<String> {
    if let Some(read) = node.as_constant_read_node() {
        return Some(text(read.name().as_slice()));
    }
    let path = node.as_constant_path_node()?;
    let name = text(path.name()?.as_slice());
    match path.parent() {
        None => Some(format!("::{name}")),
        Some(parent) => {
            if parent.as_constant_read_node().is_none() && parent.as_constant_path_node().is_none()
            {
                return None;
            }
            Some(format!("{}::{name}", constant_path_name(&parent)?))
        }
    }
}

fn constant_as_type_name(node: &Node) -> Option<TypeName> {
    if node.as_constant_read_node().is_some() || node.as_constant_path_node().is_some() {
        return constant_path_name(node).map(|name| TypeName::parse(&name));
    }
    if let Some(write) = node.as_constant_write_node() {
        return Some(TypeName {
            namespace: Namespace {
                absolute: false,
                path: Vec::new(),
            },
            name: text(write.name().as_slice()),
        });
    }
    if let Some(write) = node.as_constant_path_write_node() {
        return constant_as_type_name(&write.target().as_node());
    }
    None
}

fn builtin_instance(name: &str) -> Type {
    Type::ClassInstance {
        name: TypeName::parse(name),
        args: Vec::new(),
        location: TypeLocation::default(),
    }
}

fn constant_type(value: &Node) -> Type {
    if value.as_integer_node().is_some() {
        builtin_instance("::Integer")
    } else if value.as_float_node().is_some() {
        builtin_instance("::Float")
    } else if value.as_string_node().is_some() {
        builtin_instance("::String")
    } else if value.as_true_node().is_some() || value.as_false_node().is_some() {
        Type::Bool
    } else if value.as_symbol_node().is_some() {
        builtin_instance("::Symbol")
    } else if value.as_nil_node().is_some() {
        Type::Nil
    } else {
        Type::Any
    }
}

fn definition_parameters(node: &DefNode) -> Option<Vec<DefinitionParameter>> {
    let parameters = node.parameters()?;
    let name = |id: ruby_prism::ConstantId| text(id.as_slice());
    let mut result = Vec::new();
    for parameter in parameters.requireds().iter() {
        if let Some(required) = parameter.as_required_parameter_node() {
            result.push(DefinitionParameter::Required(name(required.name())));
        }
    }
    for parameter in parameters.optionals().iter() {
        if let Some(optional) = parameter.as_optional_parameter_node() {
            result.push(DefinitionParameter::Optional(name(optional.name())));
        }
    }
    if let Some(rest) = parameters.rest()
        && let Some(rest) = rest.as_rest_parameter_node()
    {
        result.push(DefinitionParameter::Rest(rest.name().map(name)));
    }
    for parameter in parameters.posts().iter() {
        if let Some(required) = parameter.as_required_parameter_node() {
            result.push(DefinitionParameter::Trailing(name(required.name())));
        }
    }
    for parameter in parameters.keywords().iter() {
        if let Some(keyword) = parameter.as_required_keyword_parameter_node() {
            result.push(DefinitionParameter::RequiredKeyword(name(keyword.name())));
        } else if let Some(keyword) = parameter.as_optional_keyword_parameter_node() {
            result.push(DefinitionParameter::OptionalKeyword(name(keyword.name())));
        }
    }
    if let Some(rest) = parameters.keyword_rest()
        && let Some(rest) = rest.as_keyword_rest_parameter_node()
    {
        result.push(DefinitionParameter::RestKeyword(rest.name().map(name)));
    }
    if let Some(block) = parameters.block() {
        result.push(DefinitionParameter::Block(block.name().map(name)));
    }
    Some(result)
}

impl Parser<'_> {
    fn location(&self, location: &PrismLocation) -> Option<Location> {
        self.range_location(range(location))
    }

    fn range_location(&self, range: ByteRange) -> Option<Location> {
        Some(Location::new(self.buffer, range.start, range.end))
    }

    fn start_line(&self, location: &PrismLocation) -> usize {
        self.lines.line(location.start_offset())
    }

    fn end_line(&self, location: &PrismLocation) -> usize {
        self.lines.line(location.end_offset())
    }

    fn diagnostic(&mut self, kind: DiagnosticKind, location: ByteRange, message: &str) {
        self.result.diagnostics.push(Diagnostic {
            kind,
            location,
            message: message.to_string(),
        });
    }

    fn paragraphs(&self, index: usize) -> Vec<Paragraph> {
        self.comments.block(index).each_paragraph(self.source)
    }

    fn trailing_annotation(&self, index: usize) -> Option<TrailingAnnotation> {
        self.comments.block(index).trailing_annotation()
    }

    fn skip_node(&mut self, location: &PrismLocation) -> bool {
        if let Some(index) = self.comments.leading_block(self.start_line(location))
            && self.paragraphs(index).iter().any(|paragraph| {
                matches!(
                    paragraph,
                    Paragraph::Annotation(LocatedAnnotation {
                        annotation: Annotation::Skip { .. },
                        ..
                    })
                )
            })
        {
            self.comments.associate(index);
            return true;
        }
        false
    }

    fn report_unused(&mut self, unused: Unused) {
        match unused {
            Unused::SyntaxError(error) => {
                self.diagnostic(
                    DiagnosticKind::AnnotationSyntaxError,
                    error.location,
                    &error.message,
                );
            }
            Unused::Annotation(annotation) => {
                self.diagnostic(
                    DiagnosticKind::UnusedInlineAnnotation,
                    annotation.location,
                    "Unused inline rbs annotation",
                );
            }
        }
    }

    fn report_unused_paragraph(&mut self, paragraph: Paragraph) {
        match paragraph {
            Paragraph::Text => {}
            Paragraph::Annotation(annotation) => self.report_unused(Unused::Annotation(annotation)),
            Paragraph::SyntaxError(error) => self.report_unused(Unused::SyntaxError(error)),
        }
    }

    fn report_unused_trailing(&mut self, annotation: Option<TrailingAnnotation>) {
        match annotation {
            Some(TrailingAnnotation::Annotation(annotation)) => {
                self.report_unused(Unused::Annotation(annotation))
            }
            Some(TrailingAnnotation::SyntaxError(error)) => {
                self.report_unused(Unused::SyntaxError(error))
            }
            None => {}
        }
    }

    fn report_unused_block(&mut self, index: usize) {
        if !self.comments.block(index).is_leading() {
            return;
        }
        for paragraph in self.paragraphs(index) {
            self.report_unused_paragraph(paragraph);
        }
    }

    fn insert_member(&mut self, line: usize, member: PendingMember) {
        self.module_nesting
            .last_mut()
            .expect("a member is inserted in a module")
            .members
            .push((line, member));
    }

    fn insert_declaration(&mut self, module: PendingModule) {
        match self.module_nesting.last_mut() {
            Some(current) => current
                .members
                .push((module.start_line, PendingMember::Module(module))),
            None => self.result.declarations.push(module.into_declaration()),
        }
    }

    fn insert_constant_like(&mut self, line: usize, declaration: Declaration) {
        match self.module_nesting.last_mut() {
            Some(current) => current.members.push((
                line,
                PendingMember::Member(Member::Declaration(declaration)),
            )),
            None => self.result.declarations.push(declaration),
        }
    }

    fn visit_class_or_module_body(
        &mut self,
        module: PendingModule,
        location: &PrismLocation,
        child_locations: Vec<PrismLocation>,
        visit_children: impl FnOnce(&mut Self),
    ) {
        self.module_nesting.push(module);
        visit_children(self);
        for child_location in &child_locations {
            let start_line = self.start_line(child_location);
            let end_line = self.end_line(child_location);
            for index in self
                .comments
                .enclosed_blocks_associating(start_line, end_line)
            {
                self.report_unused_block(index);
            }
        }
        let mut module = self
            .module_nesting
            .pop()
            .expect("the module is pushed above");

        let start_line = self.start_line(location);
        let end_line = self.end_line(location);
        for index in self
            .comments
            .enclosed_blocks_associating(start_line, end_line)
        {
            for paragraph in self.paragraphs(index) {
                match paragraph {
                    Paragraph::Annotation(LocatedAnnotation {
                        annotation:
                            Annotation::InstanceVariable {
                                name,
                                type_,
                                type_location,
                                ..
                            },
                        location,
                    }) => {
                        let context = self.type_context(Some(&module.name));
                        self.add_type_region(location, vec![type_location.clone()], None, context);
                        module.members.push((
                            self.lines.line(location.start),
                            PendingMember::Member(Member::Variable(Variable {
                                variable_kind: VariableKind::InstanceVariable,
                                name,
                                type_,
                                location: self.range_location(location),
                                name_location: None,
                                type_location,
                            })),
                        ))
                    }
                    paragraph => self.report_unused_paragraph(paragraph),
                }
            }
        }
        self.insert_declaration(module);
    }

    fn parse_super_class(&mut self, super_class: &Node) -> Option<Super> {
        let Some(name) = constant_as_type_name(super_class) else {
            self.diagnostic(
                DiagnosticKind::NonConstantSuperClassName,
                range(&super_class.location()),
                "Super class name must be a constant",
            );
            return None;
        };
        let mut args = Vec::new();
        let mut location = range(&super_class.location());
        let end_line = self.end_line(&super_class.location());
        if let Some(index) = self.comments.trailing_block_associating(end_line) {
            match self.trailing_annotation(index) {
                Some(TrailingAnnotation::Annotation(LocatedAnnotation {
                    annotation:
                        Annotation::TypeApplication {
                            type_arguments,
                            type_locations,
                            ..
                        },
                    location: annotation_location,
                })) => {
                    args = type_arguments;
                    location.end = annotation_location.end;
                    let context = self.type_context(None);
                    self.add_type_region(annotation_location, type_locations, None, context);
                }
                other => self.report_unused_trailing(other),
            }
        }
        Some(Super {
            name,
            args,
            location: self.range_location(location),
            name_location: None,
        })
    }

    fn parse_mixin_call(&mut self, node: &CallNode, name: &str) {
        let arguments: Vec<Node> = node
            .arguments()
            .map(|arguments| arguments.arguments().iter().collect())
            .unwrap_or_default();
        if arguments.len() > 1 {
            self.diagnostic(
                DiagnosticKind::MixinMultipleArguments,
                range(&node.location()),
                "Mixing multiple modules with one call is not supported",
            );
            return;
        }
        let Some(first_argument) = arguments.first() else {
            return;
        };
        let Some(module_name) = constant_as_type_name(first_argument) else {
            self.diagnostic(
                DiagnosticKind::MixinNonConstantModule,
                range(&first_argument.location()),
                "Module name must be a constant",
            );
            return;
        };
        let mut args = Vec::new();
        let end_line = self.end_line(&node.location());
        if let Some(index) = self.comments.trailing_block_associating(end_line) {
            match self.trailing_annotation(index) {
                Some(TrailingAnnotation::Annotation(LocatedAnnotation {
                    annotation:
                        Annotation::TypeApplication {
                            type_arguments,
                            type_locations,
                            ..
                        },
                    location: annotation_location,
                })) => {
                    args = type_arguments;
                    let context = self.type_context(None);
                    self.add_type_region(annotation_location, type_locations, None, context);
                }
                other => self.report_unused_trailing(other),
            }
        }
        let mixin_kind = match name {
            "include" => MixinKind::Include,
            "extend" => MixinKind::Extend,
            _ => MixinKind::Prepend,
        };
        let line = self.start_line(&node.location());
        self.insert_member(
            line,
            PendingMember::Member(Member::Mixin(Mixin {
                mixin_kind,
                name: module_name,
                args,
                location: self.location(&node.location()),
                name_location: None,
            })),
        );
    }

    fn parse_attribute_call(&mut self, node: &CallNode, name: &str) {
        let arguments: Vec<Node> = match node.arguments() {
            Some(arguments) => arguments.arguments().iter().collect(),
            None => return,
        };
        if arguments.is_empty() {
            return;
        }
        let mut names = Vec::new();
        for argument in &arguments {
            match argument.as_symbol_node() {
                Some(symbol) => names.push(text(symbol.unescaped())),
                None => self.diagnostic(
                    DiagnosticKind::AttributeNonSymbolName,
                    range(&argument.location()),
                    "Attribute name must be a symbol",
                ),
            }
        }
        if names.is_empty() {
            return;
        }
        let location = node.location();
        let leading_block = self
            .comments
            .leading_block_associating(self.start_line(&location));
        let mut type_ = None;
        let mut type_location = None;
        if let Some(index) = self
            .comments
            .trailing_block_associating(self.end_line(&location))
        {
            match self.trailing_annotation(index) {
                Some(TrailingAnnotation::Annotation(LocatedAnnotation {
                    annotation:
                        Annotation::NodeTypeAssertion {
                            type_: asserted,
                            type_location: asserted_location,
                            ..
                        },
                    location: annotation_location,
                })) => {
                    type_ = Some(asserted);
                    let context = self.type_context(None);
                    self.add_type_region(
                        annotation_location,
                        vec![asserted_location.clone()],
                        None,
                        context,
                    );
                    type_location = asserted_location;
                }
                Some(TrailingAnnotation::SyntaxError(error)) => self.diagnostic(
                    DiagnosticKind::AnnotationSyntaxError,
                    error.location,
                    &error.message,
                ),
                _ => {}
            }
        }
        if let Some(index) = leading_block {
            self.report_unused_block(index);
        }
        let attribute_kind = match name {
            "attr_reader" => AttributeKind::Reader,
            "attr_writer" => AttributeKind::Writer,
            _ => AttributeKind::Accessor,
        };
        let line = self.start_line(&location);
        for name in names {
            self.insert_member(
                line,
                PendingMember::Member(Member::Attribute(Attribute {
                    attribute_kind,
                    name,
                    type_: type_.clone().unwrap_or(Type::Any),
                    kind: MemberKind::Instance,
                    instance_variable: AttributeInstanceVariable::Unspecified,
                    visibility: Some(Visibility::Public),
                    annotations: Vec::new(),
                    location: self.location(&location),
                    name_location: None,
                    type_location: type_location.clone(),
                    comment: None,
                    declared_in_ruby: true,
                })),
            );
        }
    }

    fn parse_constant_declaration(
        &mut self,
        node: &Node,
        name_location: PrismLocation,
        value: Node,
    ) {
        let Some(constant_name) = constant_as_type_name(node) else {
            self.diagnostic(
                DiagnosticKind::NonConstantConstantDeclaration,
                range(&name_location),
                "Constant name must be a constant",
            );
            return;
        };
        let location = node.location();
        if let Some(index) = self
            .comments
            .leading_block_associating(self.start_line(&location))
        {
            self.report_unused_block(index);
        }
        let mut type_annotation = None;
        let mut alias_annotation = None;
        if let Some(index) = self
            .comments
            .trailing_block_associating(self.end_line(&location))
        {
            match self.trailing_annotation(index) {
                Some(TrailingAnnotation::Annotation(LocatedAnnotation {
                    annotation:
                        Annotation::NodeTypeAssertion {
                            type_,
                            type_location,
                            ..
                        },
                    location: annotation_location,
                })) => {
                    let context = self.type_context(None);
                    self.add_type_region(annotation_location, vec![type_location], None, context);
                    type_annotation = Some(type_);
                }
                Some(TrailingAnnotation::Annotation(
                    annotation @ LocatedAnnotation {
                        annotation: Annotation::ClassAlias { .. } | Annotation::ModuleAlias { .. },
                        ..
                    },
                )) => alias_annotation = Some(annotation),
                Some(TrailingAnnotation::SyntaxError(error)) => self.diagnostic(
                    DiagnosticKind::AnnotationSyntaxError,
                    error.location,
                    &error.message,
                ),
                _ => {}
            }
        }
        let line = self.start_line(&location);
        if let Some(alias_annotation) = alias_annotation {
            if let Annotation::ClassAlias {
                type_name: Some(type_name),
                type_name_location: Some(type_name_location),
                ..
            }
            | Annotation::ModuleAlias {
                type_name: Some(type_name),
                type_name_location: Some(type_name_location),
                ..
            } = &alias_annotation.annotation
            {
                let context = self.type_context(None);
                self.add_type_region(
                    alias_annotation.location,
                    Vec::new(),
                    Some((type_name.clone(), type_name_location.clone())),
                    context,
                );
            }
            let inferred_old_name = constant_as_type_name(&value);
            let (kind, type_name) = match alias_annotation.annotation {
                Annotation::ClassAlias { type_name, .. } => (ClassOrModule::Class, type_name),
                Annotation::ModuleAlias { type_name, .. } => (ClassOrModule::Module, type_name),
                _ => unreachable!("an alias annotation"),
            };
            let Some(old_name) = type_name.or(inferred_old_name) else {
                let message = match kind {
                    ClassOrModule::Class => "Class name is missing in class alias declaration",
                    ClassOrModule::Module => "Module name is missing in module alias declaration",
                };
                self.diagnostic(
                    DiagnosticKind::ClassModuleAliasDeclarationMissingTypeName,
                    alias_annotation.location,
                    message,
                );
                return;
            };
            self.insert_constant_like(
                line,
                Declaration::ClassAlias(Arc::new(AliasDeclaration {
                    kind,
                    new_name: constant_name,
                    old_name,
                    annotations: Vec::new(),
                    location: self.location(&location),
                    new_name_location: self.location(&name_location),
                    old_name_location: None,
                    comment: None,
                    declared_in_ruby: true,
                })),
            );
        } else {
            let type_ = type_annotation.unwrap_or_else(|| constant_type(&value));
            self.insert_constant_like(
                line,
                Declaration::Constant(Arc::new(ConstantDeclaration {
                    name: constant_name,
                    type_,
                    annotations: Vec::new(),
                    location: self.location(&location),
                    name_location: self.location(&name_location),
                    comment: None,
                    declared_in_ruby: true,
                })),
            );
        }
    }
}

impl<'pr> Visit<'pr> for Parser<'_> {
    fn visit_class_node(&mut self, node: &ClassNode<'pr>) {
        let location = node.location();
        if self.skip_node(&location) {
            return;
        }
        let constant_path = node.constant_path();
        let Some(class_name) = constant_as_type_name(&constant_path) else {
            self.diagnostic(
                DiagnosticKind::NonConstantClassName,
                range(&constant_path.location()),
                "Class name must be a constant",
            );
            return;
        };
        let super_class = match (node.superclass(), node.inheritance_operator_loc()) {
            (Some(super_class), Some(_)) => self.parse_super_class(&super_class),
            _ => None,
        };
        let module = PendingModule {
            kind: ModuleKind::Class { super_class },
            name: class_name,
            location: self.location(&location),
            name_location: self.location(&constant_path.location()),
            start_line: self.start_line(&location),
            members: Vec::new(),
        };
        let mut child_locations = vec![constant_path.location()];
        child_locations.extend(node.superclass().map(|super_class| super_class.location()));
        child_locations.extend(node.body().map(|body| body.location()));
        self.visit_class_or_module_body(module, &location, child_locations, |parser| {
            ruby_prism::visit_class_node(parser, node)
        });
    }

    fn visit_module_node(&mut self, node: &ModuleNode<'pr>) {
        let location = node.location();
        if self.skip_node(&location) {
            return;
        }
        let constant_path = node.constant_path();
        let Some(module_name) = constant_as_type_name(&constant_path) else {
            self.diagnostic(
                DiagnosticKind::NonConstantModuleName,
                range(&constant_path.location()),
                "Module name must be a constant",
            );
            return;
        };
        let mut module = PendingModule {
            kind: ModuleKind::Module,
            name: module_name,
            location: self.location(&location),
            name_location: self.location(&constant_path.location()),
            start_line: self.start_line(&location),
            members: Vec::new(),
        };
        if let Some(index) = self.comments.leading_block(self.start_line(&location)) {
            let mut unused = Vec::new();
            for paragraph in self.paragraphs(index) {
                match paragraph {
                    Paragraph::Annotation(LocatedAnnotation {
                        annotation:
                            Annotation::ModuleSelf {
                                name, arguments, ..
                            },
                        location,
                    }) => module.members.push((
                        self.lines.line(location.start),
                        PendingMember::ModuleSelf(ModuleSelf {
                            name,
                            args: arguments,
                            location: self.range_location(location),
                            name_location: None,
                        }),
                    )),
                    Paragraph::Text => {}
                    Paragraph::Annotation(LocatedAnnotation {
                        annotation: Annotation::Skip { .. },
                        ..
                    }) => {}
                    paragraph => unused.push(paragraph),
                }
            }
            for paragraph in unused {
                self.report_unused_paragraph(paragraph);
            }
            self.comments.associate(index);
        }
        let mut child_locations = vec![constant_path.location()];
        child_locations.extend(node.body().map(|body| body.location()));
        self.visit_class_or_module_body(module, &location, child_locations, |parser| {
            ruby_prism::visit_module_node(parser, node)
        });
    }

    fn visit_def_node(&mut self, node: &DefNode<'pr>) {
        let location = node.location();
        if self.skip_node(&location) {
            return;
        }
        if let Some(receiver) = node.receiver()
            && receiver.as_self_node().is_none()
        {
            self.diagnostic(
                DiagnosticKind::NotImplementedYet,
                range(&receiver.location()),
                "Method definition with non-self receiver is not supported",
            );
            return;
        }
        let kind = if node.receiver().is_some() {
            MethodKind::Singleton
        } else {
            MethodKind::Instance
        };
        if self.module_nesting.is_empty() {
            self.diagnostic(
                DiagnosticKind::TopLevelMethodDefinition,
                range(&node.name_loc()),
                "Top-level method definition is not supported",
            );
            return;
        }
        let leading_block = self
            .comments
            .leading_block_associating(self.start_line(&location));
        let mut trailing_block = None;
        if node.end_keyword_loc().is_some() {
            let end_location = node
                .rparen_loc()
                .or_else(|| node.parameters().map(|parameters| parameters.location()))
                .unwrap_or_else(|| node.name_loc());
            trailing_block = self
                .comments
                .trailing_block_associating(self.end_line(&end_location));
        }
        let parameters = definition_parameters(node);
        let built = MethodTypeAnnotation::build(
            leading_block.map(|index| self.paragraphs(index)),
            trailing_block.and_then(|index| self.trailing_annotation(index)),
            parameters.as_deref(),
        );
        if let Some(unused) = built.unused_trailing {
            self.report_unused(unused);
        }
        for unused in built.unused_leading {
            self.report_unused(unused);
        }
        let method_type = built.method_type;
        for (annotation, type_locations) in method_type.type_regions() {
            let context = self.type_context(None);
            self.add_type_region(annotation, type_locations, None, context);
        }
        let line = self.start_line(&location);
        self.insert_member(
            line,
            PendingMember::Member(Member::MethodDefinition(MethodDefinition {
                name: text(node.name().as_slice()),
                kind,
                overloads: method_type.overloads(),
                overloading: method_type.is_overloading(),
                visibility: Some(Visibility::Public),
                annotations: Vec::new(),
                location: self.location(&location),
                name_location: self.location(&node.name_loc()),
                comment: None,
                declared_in_ruby: true,
                unannotated_ruby_definition: method_type.is_empty(),
            })),
        );
        let end_line = self.end_line(&location);
        for index in self.comments.enclosed_blocks_associating(line, end_line) {
            self.report_unused_block(index);
        }
    }

    fn visit_call_node(&mut self, node: &CallNode<'pr>) {
        if node.receiver().is_some() {
            return;
        }
        let name = text(node.name().as_slice());
        match name.as_str() {
            "include" | "extend" | "prepend" => {
                if self.skip_node(&node.location()) {
                    return;
                }
                if !self.module_nesting.is_empty() {
                    self.parse_mixin_call(node, &name);
                }
            }
            "attr_reader" | "attr_writer" | "attr_accessor" => {
                if self.skip_node(&node.location()) {
                    return;
                }
                if self.module_nesting.is_empty() {
                    let location = node.message_loc().unwrap_or_else(|| node.location());
                    self.diagnostic(
                        DiagnosticKind::TopLevelAttributeDefinition,
                        range(&location),
                        "Top-level attribute definition is not supported",
                    );
                } else {
                    self.parse_attribute_call(node, &name);
                }
            }
            _ => ruby_prism::visit_call_node(self, node),
        }
    }

    fn visit_constant_write_node(&mut self, node: &ConstantWriteNode<'pr>) {
        if self.skip_node(&node.location()) {
            return;
        }
        self.parse_constant_declaration(&node.as_node(), node.name_loc(), node.value());
    }

    fn visit_constant_path_write_node(&mut self, node: &ConstantPathWriteNode<'pr>) {
        if self.skip_node(&node.location()) {
            return;
        }
        self.parse_constant_declaration(&node.as_node(), node.target().location(), node.value());
    }
}
