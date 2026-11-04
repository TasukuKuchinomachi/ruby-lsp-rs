use crate::ast::located_type::{self, LocatedType};

use rbs_definition::{
    ast::{
        Block, Callable, Declaration, Directive, Member, MethodType, ModuleSelf, Super, Type,
        TypeParameter, UseClause,
    },
    location::Location,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Name,
    OldName,
    NewName,
    TypeName,
    Namespace,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub enum Component<'ast> {
    Directive(&'ast Directive),
    UseClause(&'ast UseClause),
    Declaration(&'ast Declaration),
    Super(&'ast Super),
    ModuleSelf(&'ast ModuleSelf),
    Member(&'ast Member),
    MethodType(&'ast MethodType),
    TypeParameter(&'ast TypeParameter),
    Type(&'ast Type),
}

pub struct Locator<'ast> {
    pub directives: &'ast [Directive],
    pub declarations: &'ast [Declaration],
}

fn test_location(position: usize, location: Option<&Location>) -> bool {
    location.is_some_and(|location| location.start() <= position && position <= location.end())
}

fn in_key(position: usize, location: Option<&Location>) -> bool {
    location.is_some_and(|location| location.start() <= position && position < location.end())
}

fn key_in(position: usize, keys: &[(Key, Option<&Location>)]) -> Key {
    keys.iter()
        .find(|(_, location)| in_key(position, *location))
        .map_or(Key::Other, |(key, _)| *key)
}

fn declaration_location(declaration: &Declaration) -> Option<&Location> {
    match declaration {
        Declaration::Class(class) => class.location.as_ref(),
        Declaration::Module(module) => module.location.as_ref(),
        Declaration::Interface(interface) => interface.location.as_ref(),
        Declaration::TypeAlias(alias) => alias.location.as_ref(),
        Declaration::Constant(constant) => constant.location.as_ref(),
        Declaration::Global(global) => global.location.as_ref(),
        Declaration::ClassAlias(alias) => alias.location.as_ref(),
    }
}

fn member_location(member: &Member) -> Option<&Location> {
    match member {
        Member::MethodDefinition(definition) => definition.location.as_ref(),
        Member::Attribute(attribute) => attribute.location.as_ref(),
        Member::Alias(alias) => alias.location.as_ref(),
        Member::Mixin(mixin) => mixin.location.as_ref(),
        Member::Variable(variable) => variable.location.as_ref(),
        Member::Visibility(_) | Member::Declaration(_) => None,
    }
}

pub fn type_children(type_: &Type) -> Vec<&Type> {
    match type_ {
        Type::ClassInstance { args, .. }
        | Type::ClassSingleton { args, .. }
        | Type::Interface { args, .. }
        | Type::Alias { args, .. }
        | Type::Tuple(args)
        | Type::Union(args)
        | Type::Intersection(args) => args.iter().collect(),
        Type::Record(fields) => fields.iter().map(|field| &field.type_).collect(),
        Type::Optional(type_) => vec![type_],
        Type::Proc {
            function,
            block,
            self_type,
        } => {
            let mut types = callable_types(function);
            if let Some(self_type) = self_type {
                types.push(self_type);
            }
            if let Some(block) = block {
                types.extend(block_types(block));
            }
            types
        }
        _ => Vec::new(),
    }
}

fn callable_types(callable: &Callable) -> Vec<&Type> {
    match callable {
        Callable::Function(function) => function
            .required_positionals
            .iter()
            .chain(&function.optional_positionals)
            .chain(&function.rest_positionals)
            .chain(&function.trailing_positionals)
            .chain(
                function
                    .required_keywords
                    .iter()
                    .map(|(_, parameter)| parameter),
            )
            .chain(
                function
                    .optional_keywords
                    .iter()
                    .map(|(_, parameter)| parameter),
            )
            .chain(&function.rest_keywords)
            .map(|parameter| &parameter.type_)
            .chain(std::iter::once(&function.return_type))
            .collect(),
        Callable::Untyped { return_type } => vec![return_type],
    }
}

fn block_types(block: &Block) -> Vec<&Type> {
    let mut types = callable_types(&block.type_);
    if let Some(self_type) = &block.self_type {
        types.push(self_type);
    }
    types
}

pub fn method_type_types(method_type: &MethodType) -> Vec<&Type> {
    let mut types = callable_types(&method_type.type_);
    if let Some(block) = &method_type.block {
        types.extend(block_types(block));
    }
    types
}

impl<'ast> Locator<'ast> {
    pub fn new(directives: &'ast [Directive], declarations: &'ast [Declaration]) -> Self {
        Locator {
            directives,
            declarations,
        }
    }

    pub fn find(&self, position: usize) -> (Option<Key>, Vec<Component<'ast>>) {
        for directive in self.directives {
            let mut path = Vec::new();
            if let Some(key) = find_in_directive(position, directive, &mut path) {
                path.reverse();
                return (key, path);
            }
        }
        for declaration in self.declarations {
            let mut path = Vec::new();
            if let Some(key) = find_in_declaration(position, declaration, &mut path) {
                path.reverse();
                return (key, path);
            }
        }
        (None, Vec::new())
    }
}

fn find_in_directive<'ast>(
    position: usize,
    directive: &'ast Directive,
    path: &mut Vec<Component<'ast>>,
) -> Option<Option<Key>> {
    let Directive::Use(clauses, location) = directive;
    if !test_location(position, location.location().as_ref()) {
        return None;
    }
    path.push(Component::Directive(directive));
    for clause in clauses {
        let (location, keys) = match clause {
            UseClause::Single {
                location,
                type_name_location,
                ..
            } => (
                location.location(),
                vec![(Key::TypeName, type_name_location.location())],
            ),
            UseClause::Wildcard {
                location,
                namespace_location,
                ..
            } => (
                location.location(),
                vec![(Key::Namespace, namespace_location.location())],
            ),
        };
        if test_location(position, location.as_ref()) {
            path.push(Component::UseClause(clause));
            let keys: Vec<(Key, Option<&Location>)> = keys
                .iter()
                .map(|(key, location)| (*key, location.as_ref()))
                .collect();
            return Some(Some(key_in(position, &keys)));
        }
    }
    None
}

fn find_in_type_parameter<'ast>(
    position: usize,
    parameter: &'ast TypeParameter,
    path: &mut Vec<Component<'ast>>,
) -> Option<Option<Key>> {
    if !test_location(position, parameter.location.location().as_ref()) {
        return None;
    }
    path.push(Component::TypeParameter(parameter));
    if let Some(location) = parameter.location.location()
        && let Some(bounds) =
            located_type::parse_type_parameter_bounds(location.source(), location.start())
    {
        let pairs = [
            (&bounds.upper_bound, &parameter.upper_bound),
            (&bounds.lower_bound, &parameter.lower_bound),
            (&bounds.default_type, &parameter.default_type),
        ];
        for (located, type_) in pairs {
            if let (Some(located), Some(type_)) = (located, type_)
                && let Some(key) = find_in_located(position, located, type_, path)
            {
                return Some(key);
            }
        }
    }
    Some(Some(Key::Other))
}

fn find_in_located<'ast>(
    position: usize,
    located: &LocatedType,
    type_: &'ast Type,
    path: &mut Vec<Component<'ast>>,
) -> Option<Option<Key>> {
    let location = located.location;
    if !(location.begin_pos <= position && position <= location.end_pos) {
        return None;
    }
    path.push(Component::Type(type_));
    for (child_located, child) in located.children.iter().zip(type_children(type_)) {
        if let Some(key) = find_in_located(position, child_located, child, path) {
            return Some(key);
        }
    }
    let Some(name) = located.name_location else {
        return Some(None);
    };
    if location.end_pos > name.end_pos && name.end_pos <= position && position < location.end_pos {
        return Some(Some(Key::Other));
    }
    if name.begin_pos <= position && position < name.end_pos {
        return Some(Some(Key::Name));
    }
    Some(None)
}

fn located_type(location: &Location) -> Option<LocatedType> {
    located_type::parse_type(location.source(), location.start())
        .ok()
        .flatten()
}

fn find_in_type<'ast>(
    position: usize,
    type_location: Option<&Location>,
    type_: &'ast Type,
    path: &mut Vec<Component<'ast>>,
) -> Option<Option<Key>> {
    let located = located_type(type_location?)?;
    find_in_located(position, &located, type_, path)
}

fn declared_type_location(
    location: Option<&Location>,
    name_location: Option<&Location>,
) -> Option<Location> {
    let location = location?;
    let name_location = name_location?;
    let after_name = &location.buffer.content[name_location.end()..location.end()];
    let colon = after_name.find(':')?;
    Some(Location::new(
        &location.buffer,
        name_location.end() + colon + 1,
        location.end(),
    ))
}

fn find_in_method_type<'ast>(
    position: usize,
    method_type: &'ast MethodType,
    path: &mut Vec<Component<'ast>>,
) -> Option<Option<Key>> {
    if !test_location(position, method_type.location.location().as_ref()) {
        return None;
    }
    path.push(Component::MethodType(method_type));
    for parameter in &method_type.type_parameters {
        if let Some(key) = find_in_type_parameter(position, parameter, path) {
            return Some(key);
        }
    }
    if let Some(location) = method_type.location.location()
        && let Some(located) =
            located_type::parse_method_type_types(location.source(), location.start())
    {
        for (located, type_) in located.iter().zip(method_type_types(method_type)) {
            if let Some(key) = find_in_located(position, located, type_, path) {
                return Some(key);
            }
        }
    }
    Some(None)
}

fn find_in_member<'ast>(
    position: usize,
    member: &'ast Member,
    path: &mut Vec<Component<'ast>>,
) -> Option<Option<Key>> {
    let location = member_location(member);
    if !test_location(position, location) {
        return None;
    }
    path.push(Component::Member(member));
    match member {
        Member::MethodDefinition(definition) => {
            for overload in &definition.overloads {
                if let Some(key) = find_in_method_type(position, &overload.method_type, path) {
                    return Some(key);
                }
            }
            Some(Some(key_in(
                position,
                &[(Key::Name, definition.name_location.as_ref())],
            )))
        }
        Member::Variable(variable) => find_in_type(
            position,
            variable.type_location.as_ref(),
            &variable.type_,
            path,
        )
        .or_else(|| {
            Some(Some(key_in(
                position,
                &[(Key::Name, variable.name_location.as_ref())],
            )))
        }),
        Member::Attribute(attribute) => find_in_type(
            position,
            attribute.type_location.as_ref(),
            &attribute.type_,
            path,
        )
        .or_else(|| {
            Some(Some(key_in(
                position,
                &[(Key::Name, attribute.name_location.as_ref())],
            )))
        }),
        Member::Alias(alias) => Some(Some(key_in(
            position,
            &[(Key::NewName, alias.new_name_location.as_ref())],
        ))),
        Member::Mixin(mixin) => {
            let in_args = mixin.name_location.as_ref().is_some_and(|name| {
                location.is_some_and(|location| {
                    location.end() > name.end()
                        && name.end() <= position
                        && position < location.end()
                })
            });
            if in_args {
                return Some(Some(Key::Other));
            }
            Some(Some(key_in(
                position,
                &[(Key::Name, mixin.name_location.as_ref())],
            )))
        }
        Member::Visibility(_) | Member::Declaration(_) => Some(Some(Key::Other)),
    }
}

fn find_in_declaration<'ast>(
    position: usize,
    declaration: &'ast Declaration,
    path: &mut Vec<Component<'ast>>,
) -> Option<Option<Key>> {
    if !test_location(position, declaration_location(declaration)) {
        return None;
    }
    path.push(Component::Declaration(declaration));
    let (type_parameters, members, name_location): (&[TypeParameter], &[Member], _) =
        match declaration {
            Declaration::Class(class) => (
                &class.type_parameters,
                &class.members,
                class.name_location.as_ref(),
            ),
            Declaration::Module(module) => (
                &module.type_parameters,
                &module.members,
                module.name_location.as_ref(),
            ),
            Declaration::Interface(interface) => (
                &interface.type_parameters,
                &interface.members,
                interface.name_location.as_ref(),
            ),
            Declaration::TypeAlias(alias) => {
                if let Some(key) =
                    find_in_type(position, alias.type_location.as_ref(), &alias.type_, path)
                {
                    return Some(key);
                }
                return Some(Some(key_in(
                    position,
                    &[(Key::Name, alias.name_location.as_ref())],
                )));
            }
            Declaration::Constant(constant) => {
                let type_location = declared_type_location(
                    constant.location.as_ref(),
                    constant.name_location.as_ref(),
                );
                if let Some(key) =
                    find_in_type(position, type_location.as_ref(), &constant.type_, path)
                {
                    return Some(key);
                }
                return Some(Some(key_in(
                    position,
                    &[(Key::Name, constant.name_location.as_ref())],
                )));
            }
            Declaration::Global(global) => {
                let type_location =
                    declared_type_location(global.location.as_ref(), global.name_location.as_ref());
                if let Some(key) =
                    find_in_type(position, type_location.as_ref(), &global.type_, path)
                {
                    return Some(key);
                }
                return Some(Some(key_in(
                    position,
                    &[(Key::Name, global.name_location.as_ref())],
                )));
            }
            Declaration::ClassAlias(alias) => {
                return Some(Some(key_in(
                    position,
                    &[
                        (Key::NewName, alias.new_name_location.as_ref()),
                        (Key::OldName, alias.old_name_location.as_ref()),
                    ],
                )));
            }
        };
    for parameter in type_parameters {
        if let Some(key) = find_in_type_parameter(position, parameter, path) {
            return Some(key);
        }
    }
    if let Declaration::Class(class) = declaration
        && let Some(super_class) = &class.super_class
        && test_location(position, super_class.location.as_ref())
    {
        path.push(Component::Super(super_class));
        return Some(Some(super_key(
            position,
            super_class.location.as_ref(),
            super_class.name_location.as_ref(),
        )));
    }
    if let Declaration::Module(module) = declaration {
        for self_type in &module.self_types {
            if test_location(position, self_type.location.as_ref()) {
                path.push(Component::ModuleSelf(self_type));
                return Some(Some(super_key(
                    position,
                    self_type.location.as_ref(),
                    self_type.name_location.as_ref(),
                )));
            }
        }
    }
    for member in members {
        if let Member::Declaration(nested) = member
            && let Some(key) = find_in_declaration(position, nested, path)
        {
            return Some(key);
        }
    }
    for member in members {
        if !matches!(member, Member::Declaration(_))
            && let Some(key) = find_in_member(position, member, path)
        {
            return Some(key);
        }
    }
    Some(Some(key_in(position, &[(Key::Name, name_location)])))
}

fn super_key(position: usize, location: Option<&Location>, name: Option<&Location>) -> Key {
    let in_args = name.is_some_and(|name| {
        location.is_some_and(|location| {
            location.end() > name.end() && name.end() <= position && position < location.end()
        })
    });
    if in_args {
        Key::Other
    } else {
        key_in(position, &[(Key::Name, name)])
    }
}
