use std::path::{Path, PathBuf};

use rbs_definition::{
    ast::{Declaration, Member, MemberKind, MethodKind, Type as RbsType, TypeName},
    definition_builder::DefinitionMember,
    environment::ConstantEntry,
    location::Location,
};
use steep_interface::{
    steep_version::SteepVersion,
    types::{Type, literal_back_type},
};

use crate::{
    index::rbs_index::MethodDeclaration as RbsMethodDeclaration,
    locator::{self, RubyResult},
    method_name::MethodName,
    rbs_locator::{self, Component, Key},
    ruby::node::SourceRange,
    services::type_check_service::{TypeCheckService, TypeCheckedSource},
    type_inference::{constant_env::nesting_names, method_call::MethodCallKind},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuerySource {
    Ruby,
    Rbs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    Constant { name: TypeName, from: QuerySource },
    Method { name: MethodName, from: QuerySource },
    TypeName { name: TypeName },
}

#[derive(Clone, Debug)]
pub enum GotoLocation {
    Rbs(Location),
    Ruby { path: PathBuf, range: SourceRange },
}

impl GotoLocation {
    fn same(&self, other: &GotoLocation) -> bool {
        match (self, other) {
            (GotoLocation::Rbs(left), GotoLocation::Rbs(right)) => {
                std::sync::Arc::ptr_eq(&left.buffer, &right.buffer) && left.same_range(right)
            }
            (
                GotoLocation::Ruby { path, range },
                GotoLocation::Ruby {
                    path: other_path,
                    range: other_range,
                },
            ) => path == other_path && range == other_range,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GotoError {
    UnknownNode,
    Runtime(String),
}

fn runtime_error(name: &str) -> GotoError {
    GotoError::Runtime(name.to_string())
}

fn unique(locations: Vec<GotoLocation>) -> Vec<GotoLocation> {
    let mut unique: Vec<GotoLocation> = Vec::new();
    for location in locations {
        if !unique.iter().any(|existing| existing.same(&location)) {
            unique.push(location);
        }
    }
    unique
}

fn unique_queries(queries: Vec<Query>) -> Vec<Query> {
    let mut unique: Vec<Query> = Vec::new();
    for query in queries {
        if !unique.contains(&query) {
            unique.push(query);
        }
    }
    unique
}

fn test_ast_location(location: Option<SourceRange>, position: usize) -> Result<bool, GotoError> {
    let location = location.ok_or_else(|| runtime_error("NoMethodError"))?;
    Ok(location.begin_pos <= position && position <= location.end_pos)
}

pub fn each_type_name(type_: &Type, names: &mut Vec<TypeName>) {
    match type_ {
        Type::NameInstance { name, .. }
        | Type::Alias { name, .. }
        | Type::Singleton(name)
        | Type::Interface { name, .. } => names.push(name.clone()),
        Type::Literal(literal) => {
            if let Type::NameInstance { name, .. } = literal_back_type(literal) {
                names.push(name);
            }
        }
        Type::Nil => names.push(TypeName::parse("::NilClass")),
        Type::Boolean => {
            names.push(TypeName::parse("::TrueClass"));
            names.push(TypeName::parse("::FalseClass"));
        }
        _ => {}
    }
    match type_ {
        Type::NameInstance { args, .. }
        | Type::Alias { args, .. }
        | Type::Interface { args, .. }
        | Type::Union(args)
        | Type::Intersection(args)
        | Type::Tuple(args) => {
            for child in args {
                each_type_name(child, names);
            }
        }
        Type::Record(elements) => {
            for element in elements {
                each_type_name(&element.type_, names);
            }
        }
        Type::Proc {
            function,
            block,
            self_type,
        } => {
            for child in function.each_type() {
                each_type_name(child, names);
            }
            if let Some(block) = block {
                for child in block.type_.each_type() {
                    each_type_name(child, names);
                }
            }
            if let Some(self_type) = self_type {
                let mut self_children = Vec::new();
                each_type_name_of_children(self_type, &mut self_children);
                names.extend(self_children);
            }
        }
        _ => {}
    }
}

fn each_type_name_of_children(type_: &Type, names: &mut Vec<TypeName>) {
    let mut children = Vec::new();
    each_type_name(type_, &mut children);
    let own = match type_ {
        Type::NameInstance { .. }
        | Type::Alias { .. }
        | Type::Singleton(_)
        | Type::Interface { .. }
        | Type::Literal(_)
        | Type::Nil => 1,
        Type::Boolean => 2,
        _ => 0,
    };
    names.extend(children.into_iter().skip(own));
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryName {
    TypeName(TypeName),
    Method(MethodName),
}

fn parse_type_name(string: &str) -> Option<TypeName> {
    let string = if string.starts_with("::") {
        string.to_string()
    } else {
        format!("::{string}")
    };
    let type_name = TypeName::parse(&string);
    (!type_name.name.is_empty()).then_some(type_name)
}

fn find_singleton_method_dot(string: &str) -> Option<usize> {
    let characters: Vec<(usize, char)> = string.char_indices().collect();
    (1..characters.len()).rev().find_map(|index| {
        let (position, character) = characters[index];
        let previous = characters[index - 1].1;
        (character == '.' && previous != '.' && previous != ':').then_some(position)
    })
}

pub fn parse_name(string: &str) -> Option<QueryName> {
    if string.is_empty() {
        return None;
    }
    if let Some(index) = string.find('#') {
        let (type_part, method_part) = (&string[..index], &string[index + 1..]);
        if type_part.is_empty() || method_part.is_empty() {
            return None;
        }
        return Some(QueryName::Method(MethodName::instance(
            parse_type_name(type_part)?,
            method_part,
        )));
    }
    if let Some(index) = find_singleton_method_dot(string) {
        let (type_part, method_part) = (&string[..index], &string[index + 1..]);
        if type_part.is_empty() || method_part.is_empty() {
            return None;
        }
        return Some(QueryName::Method(MethodName::singleton(
            parse_type_name(type_part)?,
            method_part,
        )));
    }
    parse_type_name(string).map(QueryName::TypeName)
}

pub struct GotoService<'service> {
    pub service: &'service TypeCheckService,
}

impl<'service> GotoService<'service> {
    pub fn new(service: &'service TypeCheckService) -> GotoService<'service> {
        GotoService { service }
    }

    fn steep_version(&self) -> SteepVersion {
        self.service
            .signature_services
            .values()
            .next()
            .and_then(|service| service.latest_signatures())
            .map(|signatures| signatures.steep_version)
            .unwrap_or_default()
    }

    pub fn definition(&self, path: &Path, position: usize) -> Result<Vec<GotoLocation>, GotoError> {
        let mut locations = Vec::new();
        for query in unique_queries(self.query_at(path, position)?) {
            match query {
                Query::Constant { name, from } => {
                    if from == QuerySource::Ruby {
                        self.constant_definition_in_rbs(&name, &mut locations);
                    }
                    if from == QuerySource::Rbs {
                        self.constant_definition_in_ruby(&name, &mut locations)?;
                    }
                }
                Query::Method { name, from } => self.method_locations(
                    &name,
                    from == QuerySource::Rbs,
                    from == QuerySource::Ruby,
                    &mut locations,
                )?,
                Query::TypeName { name } => self.type_name_locations(&name, &mut locations),
            }
        }
        Ok(unique(locations))
    }

    pub fn query_definition(&self, name: &QueryName) -> Result<Vec<GotoLocation>, GotoError> {
        let mut locations = Vec::new();
        match name {
            QueryName::TypeName(name) => {
                self.constant_definition_in_rbs(name, &mut locations);
                self.constant_definition_in_ruby(name, &mut locations)?;
                self.interface_and_type_alias_locations(name, &mut locations);
            }
            QueryName::Method(name) => self.method_locations(name, true, true, &mut locations)?,
        }
        Ok(unique(locations))
    }

    fn interface_and_type_alias_locations(
        &self,
        name: &TypeName,
        locations: &mut Vec<GotoLocation>,
    ) {
        for target in &self.service.project.targets {
            let Some(environment) =
                self.service.signature_services[&target.name].latest_environment()
            else {
                continue;
            };
            if let Some(entry) = environment.interface_declarations.get(name)
                && let Some(location) = &entry.declaration.name_location
            {
                locations.push(GotoLocation::Rbs(location.clone()));
            }
            if let Some(entry) = environment.type_alias_declarations.get(name)
                && let Some(location) = &entry.declaration.name_location
            {
                locations.push(GotoLocation::Rbs(location.clone()));
            }
        }
    }

    pub fn implementation(
        &self,
        path: &Path,
        position: usize,
    ) -> Result<Vec<GotoLocation>, GotoError> {
        let mut locations = Vec::new();
        for query in unique_queries(self.query_at(path, position)?) {
            match query {
                Query::Constant { name, .. } | Query::TypeName { name } => {
                    self.constant_definition_in_ruby(&name, &mut locations)?
                }
                Query::Method { name, .. } => {
                    self.method_locations(&name, true, false, &mut locations)?
                }
            }
        }
        Ok(unique(locations))
    }

    pub fn type_definition(
        &self,
        path: &Path,
        position: usize,
    ) -> Result<Vec<GotoLocation>, GotoError> {
        let project = &self.service.project;
        if project.target_for_source_path(path).is_none()
            && project.target_for_signature_path(path).is_none()
            && project.target_for_inline_source_path(path).is_none()
        {
            return Ok(Vec::new());
        }
        if !self.service.source_files.contains_key(path) {
            return Err(runtime_error("KeyError"));
        }
        let Some(checked) = self.service.type_check_at(path, position) else {
            return Ok(Vec::new());
        };
        let Some(node) = checked
            .source
            .find_nodes(position)
            .and_then(|nodes| nodes.first().cloned())
        else {
            return Ok(Vec::new());
        };
        let type_ = checked
            .typing
            .type_of(node.id)
            .ok_or(GotoError::UnknownNode)?;
        let mut names = Vec::new();
        each_type_name(&type_, &mut names);
        let mut unique_names: Vec<TypeName> = Vec::new();
        for name in names {
            if !unique_names.contains(&name) {
                unique_names.push(name);
            }
        }
        let mut locations = Vec::new();
        for name in unique_names {
            self.type_name_locations(&name, &mut locations);
        }
        Ok(unique(locations))
    }

    pub fn query_at(&self, path: &Path, position: usize) -> Result<Vec<Query>, GotoError> {
        let project = &self.service.project;
        let mut queries = Vec::new();
        if project.target_for_inline_source_path(path).is_some() {
            if let Some((name, _)) = self.service.inline_type_name_at(path, position) {
                queries.push(Query::TypeName { name });
            }
            if queries.is_empty() {
                if !self.service.source_files.contains_key(path) {
                    return Ok(Vec::new());
                }
                if let Some(checked) = self.service.type_check_at(path, position) {
                    queries.extend(self.query_at_implementation(&checked, position)?);
                }
            }
        } else if project.target_for_source_path(path).is_some() {
            if !self.service.source_files.contains_key(path) {
                return Ok(Vec::new());
            }
            if let Some(checked) = self.service.type_check_at(path, position) {
                queries.extend(self.query_at_implementation(&checked, position)?);
            }
        } else {
            let target_names = self.service.signature_file_targets(path);
            for target_name in target_names {
                let Some(environment) =
                    self.service.signature_services[&target_name].latest_environment()
                else {
                    continue;
                };
                let absolute_path = project.absolute_path(path);
                let source = environment
                    .sources
                    .iter()
                    .find(|source| source.path == absolute_path || source.path == path)
                    .ok_or_else(|| runtime_error("RuntimeError"))?;
                let locator = rbs_locator::Locator::new(&source.directives, &source.declarations);
                let (key, components) = locator.find(position);
                let Some(head) = components.first() else {
                    return Err(runtime_error("RuntimeError"));
                };
                if key != Some(Key::Name) {
                    continue;
                }
                match head {
                    Component::Declaration(Declaration::Class(class)) => {
                        queries.push(Query::Constant {
                            name: class.name.clone(),
                            from: QuerySource::Rbs,
                        })
                    }
                    Component::Declaration(Declaration::Module(module)) => {
                        queries.push(Query::Constant {
                            name: module.name.clone(),
                            from: QuerySource::Rbs,
                        })
                    }
                    Component::Declaration(Declaration::Constant(constant)) => {
                        queries.push(Query::Constant {
                            name: constant.name.clone(),
                            from: QuerySource::Rbs,
                        })
                    }
                    Component::Member(Member::MethodDefinition(definition)) => {
                        let type_name = match components.get(1) {
                            Some(Component::Declaration(Declaration::Class(class))) => &class.name,
                            Some(Component::Declaration(Declaration::Module(module))) => {
                                &module.name
                            }
                            Some(Component::Declaration(Declaration::Interface(interface))) => {
                                &interface.name
                            }
                            _ => return Err(runtime_error("NoMethodError")),
                        };
                        if matches!(
                            definition.kind,
                            MethodKind::Instance | MethodKind::SingletonInstance
                        ) {
                            queries.push(Query::Method {
                                name: MethodName::instance(type_name.clone(), &definition.name),
                                from: QuerySource::Rbs,
                            });
                        }
                        if matches!(
                            definition.kind,
                            MethodKind::Singleton | MethodKind::SingletonInstance
                        ) {
                            queries.push(Query::Method {
                                name: MethodName::singleton(type_name.clone(), &definition.name),
                                from: QuerySource::Rbs,
                            });
                        }
                    }
                    Component::Member(Member::Mixin(mixin)) => queries.push(Query::TypeName {
                        name: mixin.name.clone(),
                    }),
                    Component::Type(
                        RbsType::ClassInstance { name, .. }
                        | RbsType::ClassSingleton { name, .. }
                        | RbsType::Interface { name, .. }
                        | RbsType::Alias { name, .. },
                    ) => queries.push(Query::TypeName { name: name.clone() }),
                    Component::Super(super_class) => queries.push(Query::TypeName {
                        name: super_class.name.clone(),
                    }),
                    Component::ModuleSelf(self_type) => queries.push(Query::TypeName {
                        name: self_type.name.clone(),
                    }),
                    _ => {}
                }
            }
        }
        Ok(queries)
    }

    fn query_at_implementation(
        &self,
        checked: &TypeCheckedSource,
        position: usize,
    ) -> Result<Vec<Query>, GotoError> {
        let mut queries = Vec::new();
        let typing = &checked.typing;
        let locator = locator::Ruby::new(&checked.source);
        match locator.find(position) {
            Some(RubyResult::Node(result)) => {
                let node = &result.node;
                match node.type_ {
                    "const" | "casgn" => {
                        if test_ast_location(node.location.name, position)?
                            && let Some(name) = typing.source_index().reference(node.id).cloned()
                        {
                            queries.push(Query::Constant {
                                name,
                                from: QuerySource::Ruby,
                            });
                        }
                    }
                    "def" | "defs" => {
                        if test_ast_location(node.location.name, position)?
                            && let Some(context) = typing.cursor_context().context()
                            && let Some(method_context) = &context.method_context
                            && let Some(method) = &method_context.method
                        {
                            let method_name = method_context.name.clone().unwrap_or_default();
                            for definition in &method.defs {
                                let singleton = match &definition.member {
                                    DefinitionMember::MethodDefinition(member) => {
                                        member.kind == MethodKind::Singleton
                                    }
                                    DefinitionMember::Attribute(member) => {
                                        member.kind == MemberKind::Singleton
                                    }
                                };
                                let name = if singleton {
                                    MethodName::singleton(
                                        definition.defined_in.clone(),
                                        method_name.clone(),
                                    )
                                } else {
                                    MethodName::instance(
                                        definition.defined_in.clone(),
                                        method_name.clone(),
                                    )
                                };
                                queries.push(Query::Method {
                                    name,
                                    from: QuerySource::Ruby,
                                });
                            }
                        }
                    }
                    "send" if test_ast_location(node.location.selector, position)? => {
                        let block_types: &[&str] =
                            if self.steep_version() >= SteepVersion::Version2_1_0 {
                                &["block", "numblock", "itblock"]
                            } else {
                                &["block"]
                            };
                        let call_node = match result.parents.first() {
                            Some(parent)
                                if block_types.contains(&parent.type_)
                                    && parent.child_node(0).is_some_and(|child| child == node) =>
                            {
                                parent.clone()
                            }
                            _ => node.clone(),
                        };
                        let call = typing.call_of(call_node.id).ok_or(GotoError::UnknownNode)?;
                        if let MethodCallKind::Typed { .. }
                        | MethodCallKind::Special { .. }
                        | MethodCallKind::Error { .. } = call.kind
                        {
                            for declaration in &call.method_decls {
                                queries.push(Query::Method {
                                    name: declaration.name(),
                                    from: QuerySource::Ruby,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some(RubyResult::TypeAssertion { assertion, .. }) => {
                let cursor_context = typing.cursor_context();
                let context = cursor_context
                    .context()
                    .ok_or_else(|| runtime_error("RuntimeError"))?;
                let nesting = nesting_names(&context.module_context.nesting);
                if let Some(rbs_type) = assertion
                    .rbs_type(&nesting, &checked.checker)
                    .ok()
                    .flatten()
                    && let Some((name, _)) = locator::type_name_at(position, &rbs_type)
                {
                    queries.push(Query::TypeName { name });
                }
            }
            Some(RubyResult::TypeApplication { application, .. }) => {
                let cursor_context = typing.cursor_context();
                let context = cursor_context
                    .context()
                    .ok_or_else(|| runtime_error("RuntimeError"))?;
                let nesting = nesting_names(&context.module_context.nesting);
                if let Ok(types) = application.rbs_types(&nesting, &checked.checker) {
                    for rbs_type in types {
                        if rbs_type.location.begin_pos <= position
                            && position <= rbs_type.location.end_pos
                        {
                            if let Some((name, _)) = locator::type_name_at(position, &rbs_type) {
                                queries.push(Query::TypeName { name });
                            }
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(queries)
    }

    fn constant_definition_in_rbs(&self, name: &TypeName, locations: &mut Vec<GotoLocation>) {
        for target in &self.service.project.targets {
            let Some(environment) =
                self.service.signature_services[&target.name].latest_environment()
            else {
                continue;
            };
            match environment.constant_entry(name) {
                Some(ConstantEntry::Constant(entry)) => {
                    if let Some(location) = &entry.declaration.name_location {
                        locations.push(GotoLocation::Rbs(location.clone()));
                    }
                }
                Some(ConstantEntry::Class(entry)) => {
                    for declaration in entry.declarations() {
                        if let Some(location) = declaration.name_location() {
                            locations.push(GotoLocation::Rbs(location.clone()));
                        }
                    }
                }
                Some(ConstantEntry::ClassAlias(entry)) => {
                    if let Some(location) = &entry.declaration.new_name_location {
                        locations.push(GotoLocation::Rbs(location.clone()));
                    }
                }
                None => {}
            }
        }
    }

    fn source_target_for_definition(&self, path: &Path) -> Result<bool, GotoError> {
        if self.service.project.target_for_source_path(path).is_some() {
            return Ok(true);
        }
        if self.steep_version() >= SteepVersion::Version2_1_0 {
            Ok(false)
        } else {
            Err(runtime_error("RuntimeError"))
        }
    }

    fn constant_definition_in_ruby(
        &self,
        name: &TypeName,
        locations: &mut Vec<GotoLocation>,
    ) -> Result<(), GotoError> {
        for (path, file) in &self.service.source_files {
            let Some(index) = &file.source_index else {
                continue;
            };
            if !self.source_target_for_definition(path)? {
                continue;
            }
            let Some(entry) = index.constant_entry(name) else {
                continue;
            };
            for node in &entry.definitions {
                let range = match node.type_ {
                    "const" => node.location.expression,
                    "casgn" => match (node.scope_expression, node.location.name) {
                        (Some(scope), Some(name)) => Some(scope.join(name)),
                        (None, name) => name,
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(range) = range {
                    locations.push(GotoLocation::Ruby {
                        path: path.clone(),
                        range,
                    });
                }
            }
        }
        Ok(())
    }

    fn method_locations(
        &self,
        name: &MethodName,
        in_ruby: bool,
        in_rbs: bool,
        locations: &mut Vec<GotoLocation>,
    ) -> Result<(), GotoError> {
        let initialize = match name {
            MethodName::Singleton {
                type_name,
                method_name,
            } if method_name == "new" => {
                Some(MethodName::instance(type_name.clone(), "initialize"))
            }
            _ => None,
        };
        if in_ruby {
            for (path, file) in &self.service.source_files {
                let Some(index) = &file.source_index else {
                    continue;
                };
                if !self.source_target_for_definition(path)? {
                    continue;
                }
                let mut definitions = index
                    .method_entry(name)
                    .map(|entry| entry.definitions.clone())
                    .unwrap_or_default();
                if definitions.is_empty()
                    && let Some(initialize) = &initialize
                {
                    definitions = index
                        .method_entry(initialize)
                        .map(|entry| entry.definitions.clone())
                        .unwrap_or_default();
                }
                for node in definitions {
                    if matches!(node.type_, "def" | "defs")
                        && let Some(range) = node.location.name
                    {
                        locations.push(GotoLocation::Ruby {
                            path: path.clone(),
                            range,
                        });
                    }
                }
            }
        }
        if in_rbs {
            for target in &self.service.project.targets {
                let Some(signatures) =
                    self.service.signature_services[&target.name].latest_signatures()
                else {
                    continue;
                };
                let index = signatures.rbs_index();
                let mut declarations = index.method_declarations(name);
                if declarations.is_empty()
                    && let Some(initialize) = &initialize
                {
                    declarations = index.method_declarations(initialize);
                }
                for declaration in declarations {
                    let location = match declaration {
                        RbsMethodDeclaration::MethodDefinition(definition) => {
                            definition.name_location.as_ref()
                        }
                        RbsMethodDeclaration::Alias(alias) => alias.new_name_location.as_ref(),
                        RbsMethodDeclaration::Attribute(attribute) => {
                            attribute.name_location.as_ref()
                        }
                    };
                    if let Some(location) = location {
                        locations.push(GotoLocation::Rbs(location.clone()));
                    }
                }
            }
        }
        Ok(())
    }

    fn type_name_locations(&self, name: &TypeName, locations: &mut Vec<GotoLocation>) {
        for target in &self.service.project.targets {
            let Some(signatures) =
                self.service.signature_services[&target.name].latest_signatures()
            else {
                continue;
            };
            for declaration in signatures.rbs_index().type_declarations(name) {
                let location = match declaration {
                    Declaration::Class(class) => class.name_location.as_ref(),
                    Declaration::Module(module) => module.name_location.as_ref(),
                    Declaration::Interface(interface) => interface.name_location.as_ref(),
                    Declaration::TypeAlias(alias) => alias.name_location.as_ref(),
                    Declaration::ClassAlias(alias) => alias.new_name_location.as_ref(),
                    Declaration::Constant(_) | Declaration::Global(_) => None,
                };
                if let Some(location) = location {
                    locations.push(GotoLocation::Rbs(location.clone()));
                }
            }
        }
    }
}
