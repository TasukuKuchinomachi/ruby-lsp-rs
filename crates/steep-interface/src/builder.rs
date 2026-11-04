use std::{cell::RefCell, fmt, rc::Rc, sync::Arc};

use rbs_definition::{
    ast::{Literal, MethodKind, TypeName, Variance, Visibility},
    definition_builder::{Definition, DefinitionBuilder, DefinitionMember, Method, TypeDefinition},
    environment::Environment,
};
use rbs_definition::{collections::HashMap, errors::RbsError};

use crate::{
    factory::Factory,
    method_type::{Block, Function, MethodType, Params, TypeParam},
    steep_version::SteepVersion,
    substitution::Substitution,
    subtyping::check::{Check, CheckState},
    types::{Logic, Type, literal_back_type},
};

#[derive(Clone, Debug)]
pub struct Overload {
    pub method_type: MethodType,
    pub definitions: Vec<Rc<TypeDefinition>>,
}

impl Overload {
    fn new(method_type: MethodType) -> Overload {
        Overload {
            method_type,
            definitions: Vec::new(),
        }
    }

    fn substitute(&self, substitution: &Substitution) -> Overload {
        Overload {
            method_type: self.method_type.substitute(substitution),
            definitions: self.definitions.clone(),
        }
    }
}

fn same_definition(left: &TypeDefinition, right: &TypeDefinition) -> bool {
    let same_member = match (&left.member, &right.member) {
        (DefinitionMember::MethodDefinition(left), DefinitionMember::MethodDefinition(right)) => {
            Arc::ptr_eq(left, right)
        }
        (DefinitionMember::Attribute(left), DefinitionMember::Attribute(right)) => {
            Arc::ptr_eq(left, right)
        }
        _ => false,
    };
    same_member
        && left.type_ == right.type_
        && left.defined_in == right.defined_in
        && left.implemented_in == right.implemented_in
}

fn same_definitions(left: &[Rc<TypeDefinition>], right: &[Rc<TypeDefinition>]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| same_definition(left, right))
}

fn combined_definitions(
    left: &[Rc<TypeDefinition>],
    right: &[Rc<TypeDefinition>],
) -> Vec<Rc<TypeDefinition>> {
    let mut definitions: Vec<Rc<TypeDefinition>> = Vec::new();
    for definition in left.iter().chain(right) {
        if !definitions
            .iter()
            .any(|existing| same_definition(existing, definition))
        {
            definitions.push(definition.clone());
        }
    }
    definitions
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub method_name: String,
    pub private_method: bool,
    pub overloads: Vec<Overload>,
}

impl Entry {
    pub fn method_types(&self) -> Vec<&MethodType> {
        self.overloads
            .iter()
            .map(|overload| &overload.method_type)
            .collect()
    }
}

impl fmt::Display for Entry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let method_types: Vec<String> = self
            .method_types()
            .iter()
            .map(ToString::to_string)
            .collect();
        write!(formatter, "{{ {} }}", method_types.join(" || "))
    }
}

#[derive(Debug)]
enum LazyOverloads {
    Resolved(Option<Vec<Overload>>),
    Union(Vec<MemberOverloads>),
}

// Why not resolve every method of every member when building a union shape: a union
// with a Rails model has thousands of methods, and a check calls only a few of them.
#[derive(Clone, Debug)]
enum MemberOverloads {
    Resolved(Vec<Overload>),
    Method(Shape),
}

#[derive(Clone, Debug)]
struct RawEntry {
    method_name: String,
    private_method: bool,
    overloads: Rc<RefCell<LazyOverloads>>,
}

impl RawEntry {
    fn resolved(entry: Entry) -> RawEntry {
        RawEntry {
            method_name: entry.method_name,
            private_method: entry.private_method,
            overloads: Rc::new(RefCell::new(LazyOverloads::Resolved(Some(entry.overloads)))),
        }
    }

    fn public(method_name: &str, overloads: Vec<Overload>) -> RawEntry {
        RawEntry::resolved(Entry {
            method_name: method_name.to_string(),
            private_method: false,
            overloads,
        })
    }

    fn force(&self, builder: &Builder) -> Result<Option<Vec<Overload>>, RbsError> {
        let members = match &*self.overloads.borrow() {
            LazyOverloads::Resolved(overloads) => return Ok(overloads.clone()),
            LazyOverloads::Union(members) => members.clone(),
        };
        let mut pending = Vec::new();
        for member in members {
            pending.push(match member {
                MemberOverloads::Resolved(overloads) => overloads,
                MemberOverloads::Method(shape) => {
                    shape
                        .method(builder, &self.method_name)?
                        .expect("a member of a union has the common method")
                        .overloads
                }
            });
        }
        let overloads = builder.union_overloads(&pending)?;
        *self.overloads.borrow_mut() = LazyOverloads::Resolved(overloads.clone());
        Ok(overloads)
    }
}

#[derive(Debug)]
struct DefinitionMethods {
    definition: Arc<Definition>,
    class_type: Option<Type>,
    converted: RefCell<HashMap<String, RawEntry>>,
}

#[derive(Clone, Debug)]
enum RawMethods {
    Entries {
        entries: Vec<RawEntry>,
        indexes: HashMap<String, usize>,
    },
    Definition(Rc<DefinitionMethods>),
}

impl Default for RawMethods {
    fn default() -> RawMethods {
        RawMethods::Entries {
            entries: Vec::new(),
            indexes: HashMap::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RawShape {
    methods: RawMethods,
    public_only: bool,
}

impl RawShape {
    fn of_definition(definition: Arc<Definition>, class_type: Option<Type>) -> RawShape {
        RawShape {
            methods: RawMethods::Definition(Rc::new(DefinitionMethods {
                definition,
                class_type,
                converted: RefCell::default(),
            })),
            public_only: false,
        }
    }

    fn get(&self, builder: &Builder, method_name: &str) -> Option<RawEntry> {
        let entry = match &self.methods {
            RawMethods::Entries { entries, indexes } => indexes
                .get(method_name)
                .map(|index| entries[*index].clone()),
            RawMethods::Definition(methods) => {
                let method = methods.definition.methods.get(method_name)?;
                Some(builder.converted_entry(methods, method_name, method))
            }
        }?;
        (!self.public_only || !entry.private_method).then_some(entry)
    }

    fn entries(&self, builder: &Builder) -> Vec<RawEntry> {
        let entries = match &self.methods {
            RawMethods::Entries { entries, .. } => entries.clone(),
            RawMethods::Definition(methods) => methods
                .definition
                .methods
                .iter()
                .map(|(method_name, method)| builder.converted_entry(methods, method_name, method))
                .collect(),
        };
        entries
            .into_iter()
            .filter(|entry| !self.public_only || !entry.private_method)
            .collect()
    }

    fn method_names(&self) -> Vec<String> {
        match &self.methods {
            RawMethods::Entries { entries, .. } => entries
                .iter()
                .filter(|entry| !self.public_only || !entry.private_method)
                .map(|entry| entry.method_name.clone())
                .collect(),
            RawMethods::Definition(methods) => methods
                .definition
                .methods
                .iter()
                .filter(|(_, method)| {
                    !self.public_only || method.accessibility != Visibility::Private
                })
                .map(|(method_name, _)| method_name.clone())
                .collect(),
        }
    }

    fn insert(&mut self, entry: RawEntry) {
        let RawMethods::Entries { entries, indexes } = &mut self.methods else {
            unreachable!("a shape of a definition is never modified");
        };
        match indexes.get(&entry.method_name) {
            Some(index) => entries[*index] = entry,
            None => {
                indexes.insert(entry.method_name.clone(), entries.len());
                entries.push(entry);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub type_: Type,
    raw: Rc<RawShape>,
    substitutions: Vec<Substitution>,
}

impl Shape {
    fn new(type_: Type, raw: Rc<RawShape>) -> Shape {
        Shape {
            type_,
            raw,
            substitutions: Vec::new(),
        }
    }

    fn substitute(&self, substitution: Substitution, type_: Option<Type>) -> Shape {
        let mut substitutions = self.substitutions.clone();
        substitutions.push(substitution.clone());
        Shape {
            type_: type_.unwrap_or_else(|| self.type_.substitute(&substitution)),
            raw: self.raw.clone(),
            substitutions,
        }
    }

    fn merged_substitution(&self) -> Substitution {
        self.substitutions
            .iter()
            .fold(Substitution::empty(), |merged, substitution| {
                merged.merge(substitution, true)
            })
    }

    fn resolve(
        &self,
        builder: &Builder,
        entry: &RawEntry,
        substitution: &Substitution,
    ) -> Result<Option<Entry>, RbsError> {
        Ok(entry.force(builder)?.map(|overloads| Entry {
            method_name: entry.method_name.clone(),
            private_method: entry.private_method,
            overloads: overloads
                .iter()
                .map(|overload| overload.substitute(substitution))
                .collect(),
        }))
    }

    pub fn method_names(&self) -> Vec<String> {
        self.raw.method_names()
    }

    pub fn method(&self, builder: &Builder, method_name: &str) -> Result<Option<Entry>, RbsError> {
        match self.raw.get(builder, method_name) {
            Some(entry) => self.resolve(builder, &entry, &self.merged_substitution()),
            None => Ok(None),
        }
    }

    pub fn methods(&self, builder: &Builder) -> Result<Vec<Entry>, RbsError> {
        let substitution = self.merged_substitution();
        let mut entries = Vec::new();
        for entry in self.raw.entries(builder) {
            if let Some(entry) = self.resolve(builder, &entry, &substitution)? {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    pub fn public_shape(&self) -> Shape {
        Shape {
            type_: self.type_.clone(),
            raw: Rc::new(RawShape {
                methods: self.raw.methods.clone(),
                public_only: true,
            }),
            substitutions: self.substitutions.clone(),
        }
    }

    fn resolved(&self, builder: &Builder) -> Result<RawShape, RbsError> {
        let mut raw = RawShape::default();
        for (method_name, private_method, member) in self.member_methods(builder)? {
            raw.insert(RawEntry {
                method_name,
                private_method,
                overloads: Rc::new(RefCell::new(LazyOverloads::Union(vec![member]))),
            });
        }
        Ok(raw)
    }

    fn member_methods(
        &self,
        builder: &Builder,
    ) -> Result<Vec<(String, bool, MemberOverloads)>, RbsError> {
        match &self.raw.methods {
            RawMethods::Definition(methods) => Ok(methods
                .definition
                .methods
                .iter()
                .map(|(method_name, method)| {
                    (method_name, method.accessibility == Visibility::Private)
                })
                .filter(|(_, private_method)| !self.raw.public_only || !private_method)
                .map(|(method_name, private_method)| {
                    (
                        method_name.clone(),
                        private_method,
                        MemberOverloads::Method(self.clone()),
                    )
                })
                .collect()),
            RawMethods::Entries { .. } => Ok(self
                .methods(builder)?
                .into_iter()
                .map(|entry| {
                    (
                        entry.method_name,
                        entry.private_method,
                        MemberOverloads::Resolved(entry.overloads),
                    )
                })
                .collect()),
        }
    }
}

#[derive(Default)]
pub struct Config {
    pub self_type: Option<Type>,
    pub class_type: Option<Type>,
    pub instance_type: Option<Type>,
    pub variable_bounds: HashMap<String, Type>,
}

impl Config {
    fn substitution(&self) -> Option<Substitution> {
        if self.self_type.is_none() && self.class_type.is_none() && self.instance_type.is_none() {
            return None;
        }
        Some(Substitution::build(
            &[],
            &[],
            self.instance_type.clone(),
            self.class_type.clone(),
            self.self_type.clone(),
        ))
    }
}

pub struct Builder<'environment> {
    environment: &'environment Environment,
    pub definition_builder: Arc<DefinitionBuilder<'environment>>,
    pub factory: Factory<'environment>,
    pub implicitly_returns_nil: bool,
    pub steep_version: SteepVersion,
    object_shape_cache: RefCell<HashMap<TypeName, Rc<RawShape>>>,
    union_shape_cache: RefCell<HashMap<Type, Shape>>,
    singleton_shape_cache: RefCell<HashMap<TypeName, Rc<RawShape>>>,
    subtyping: Rc<CheckState>,
}

fn name(string: &str) -> TypeName {
    TypeName::parse(string)
}

fn instance_type(name_string: &str, args: Vec<Type>) -> Type {
    Type::NameInstance {
        name: name(name_string),
        args,
    }
}

fn method_type(params: Params, return_type: Type) -> MethodType {
    MethodType {
        type_params: Vec::new(),
        type_: Function {
            params: Some(params),
            return_type,
        },
        block: None,
    }
}

fn required(types: Vec<Type>) -> Params {
    Params::required(types)
}

fn type_parameter_t() -> TypeParam {
    TypeParam {
        name: "T".to_string(),
        upper_bound: None,
        variance: Variance::Invariant,
        unchecked: false,
        default_type: None,
    }
}

fn variable_t() -> Type {
    Type::Var("T".to_string())
}

fn index_literal(index: usize) -> Type {
    Type::Literal(Literal::Integer(index.to_string()))
}

fn contains_self(type_: &Type) -> bool {
    match type_ {
        Type::SelfType => true,
        Type::NameInstance { args, .. }
        | Type::Interface { args, .. }
        | Type::Alias { args, .. } => args.iter().any(contains_self),
        Type::Union(types) | Type::Intersection(types) | Type::Tuple(types) => {
            types.iter().any(contains_self)
        }
        Type::Record(elements) => elements.iter().any(|element| contains_self(&element.type_)),
        Type::Proc {
            function,
            block,
            self_type,
        } => {
            function_contains_self(function)
                || block
                    .as_ref()
                    .is_some_and(|block| block_contains_self(block))
                || self_type.as_ref().is_some_and(|type_| contains_self(type_))
        }
        _ => false,
    }
}

fn function_contains_self(function: &Function) -> bool {
    function
        .params
        .as_ref()
        .is_some_and(|params| params.each_type().any(contains_self))
        || contains_self(&function.return_type)
}

fn block_contains_self(block: &Block) -> bool {
    function_contains_self(&block.type_) || block.self_type.as_ref().is_some_and(contains_self)
}

fn is_instance_member(type_definition: &TypeDefinition) -> bool {
    match &type_definition.member {
        DefinitionMember::MethodDefinition(definition) => definition.kind != MethodKind::Singleton,
        DefinitionMember::Attribute(_) => false,
    }
}

fn replace_primitive_method(
    method_name: &str,
    type_definition: &TypeDefinition,
    method_type: MethodType,
) -> MethodType {
    let DefinitionMember::MethodDefinition(member) = &type_definition.member else {
        return method_type;
    };
    if member.declared_in_ruby {
        return method_type;
    }
    let defined_in = &type_definition.defined_in;
    let defined_in_one_of =
        |names: &[&str]| names.iter().any(|candidate| *defined_in == name(candidate));
    const EQUALITY_CLASSES: &[&str] = &[
        "::BasicObject",
        "::Object",
        "::Kernel",
        "::String",
        "::Integer",
        "::Symbol",
        "::TrueClass",
        "::FalseClass",
        "::NilClass",
    ];
    let logic = match method_name {
        "is_a?" | "kind_of?" | "instance_of?" if is_instance_member(type_definition) => {
            Some(Logic::ReceiverIsArg)
        }
        "nil?"
            if defined_in_one_of(&["::Object", "::NilClass", "::Kernel"])
                && is_instance_member(type_definition) =>
        {
            Some(Logic::ReceiverIsNil)
        }
        "!" if defined_in_one_of(&[
            "::BasicObject",
            "::TrueClass",
            "::FalseClass",
            "::NilClass",
        ]) =>
        {
            Some(Logic::Not)
        }
        "===" if defined_in_one_of(&["::Module"]) => Some(Logic::ArgIsReceiver),
        "===" if defined_in_one_of(EQUALITY_CLASSES) => Some(Logic::ArgEqualsReceiver),
        "==" if defined_in_one_of(EQUALITY_CLASSES) => Some(Logic::ReceiverIsArg),
        "<" | "<=" if defined_in_one_of(&["::Module"]) => Some(Logic::ArgIsAncestor),
        _ => None,
    };
    match logic {
        Some(logic) => method_type.with_return_type(Type::Logic(logic)),
        None => method_type,
    }
}

fn replace_kernel_class(
    method_name: &str,
    type_definition: &TypeDefinition,
    method_type: MethodType,
    class_type: Type,
) -> MethodType {
    match &type_definition.member {
        DefinitionMember::MethodDefinition(member)
            if !member.declared_in_ruby
                && method_name == "class"
                && type_definition.defined_in == name("::Kernel") =>
        {
            method_type.with_return_type(class_type)
        }
        _ => method_type,
    }
}

impl<'environment> Builder<'environment> {
    pub fn new(environment: &'environment Environment) -> Builder<'environment> {
        Builder::with_definition_builder(environment, Arc::new(DefinitionBuilder::new(environment)))
    }

    pub fn with_definition_builder(
        environment: &'environment Environment,
        definition_builder: Arc<DefinitionBuilder<'environment>>,
    ) -> Builder<'environment> {
        Builder {
            environment,
            definition_builder,
            factory: Factory::new(environment),
            implicitly_returns_nil: false,
            steep_version: SteepVersion::default(),
            object_shape_cache: RefCell::default(),
            union_shape_cache: RefCell::default(),
            singleton_shape_cache: RefCell::default(),
            subtyping: Rc::default(),
        }
    }

    pub fn subtyping(&self) -> Check<'_, 'environment> {
        Check::with_state(self, self.subtyping.clone())
    }

    fn add_implicitly_returns_nil(
        &self,
        type_definition: &TypeDefinition,
        method_type: MethodType,
    ) -> MethodType {
        if !self.implicitly_returns_nil {
            return method_type;
        }
        let annotated = type_definition
            .member_annotations
            .iter()
            .chain(&type_definition.overload_annotations)
            .any(|annotation| annotation == "implicitly-returns-nil");
        if annotated {
            let return_type = Type::union(vec![method_type.type_.return_type.clone(), Type::Nil]);
            method_type.with_return_type(return_type)
        } else {
            method_type
        }
    }

    fn converted_entry(
        &self,
        methods: &DefinitionMethods,
        method_name: &str,
        method: &Method,
    ) -> RawEntry {
        if let Some(entry) = methods.converted.borrow().get(method_name) {
            return entry.clone();
        }
        let overloads = method
            .defs
            .iter()
            .map(|type_definition| {
                let method_type = self.factory.method_type(&type_definition.type_);
                let method_type =
                    replace_primitive_method(method_name, type_definition, method_type);
                let method_type = match &methods.class_type {
                    Some(class_type) => replace_kernel_class(
                        method_name,
                        type_definition,
                        method_type,
                        class_type.clone(),
                    ),
                    None => method_type,
                };
                Overload {
                    method_type: self.add_implicitly_returns_nil(type_definition, method_type),
                    definitions: vec![Rc::new(type_definition.clone())],
                }
            })
            .collect();
        let entry = RawEntry::resolved(Entry {
            method_name: method_name.to_string(),
            private_method: method.accessibility == Visibility::Private,
            overloads,
        });
        methods
            .converted
            .borrow_mut()
            .insert(method_name.to_string(), entry.clone());
        entry
    }

    fn object_shape(&self, type_name: &TypeName) -> Result<Rc<RawShape>, RbsError> {
        if let Some(shape) = self.object_shape_cache.borrow().get(type_name) {
            return Ok(shape.clone());
        }
        let (definition, class_type) = if type_name.is_class() {
            (
                self.definition_builder.build_instance(type_name)?,
                Some(Type::Singleton(type_name.clone())),
            )
        } else {
            (self.definition_builder.build_interface(type_name)?, None)
        };
        let shape = Rc::new(RawShape::of_definition(definition, class_type));
        self.object_shape_cache
            .borrow_mut()
            .insert(type_name.clone(), shape.clone());
        Ok(shape)
    }

    fn singleton_shape(&self, type_name: &TypeName) -> Result<Rc<RawShape>, RbsError> {
        if let Some(shape) = self.singleton_shape_cache.borrow().get(type_name) {
            return Ok(shape.clone());
        }
        let definition = self.definition_builder.build_singleton(type_name)?;
        let shape = Rc::new(RawShape::of_definition(
            definition,
            Some(instance_type("::Class", Vec::new())),
        ));
        self.singleton_shape_cache
            .borrow_mut()
            .insert(type_name.clone(), shape.clone());
        Ok(shape)
    }

    fn app_substitution(&self, type_: &Type) -> Substitution {
        let (name, args) = match type_ {
            Type::NameInstance { name, args } | Type::Interface { name, args } => (name, args),
            _ => return Substitution::empty(),
        };
        if args.is_empty() {
            return Substitution::empty();
        }
        let variables: Vec<String> = if name.is_class() {
            self.environment.class_declarations[&self.environment.normalize_module_name(name)]
                .primary_declaration()
                .type_parameters()
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect()
        } else {
            self.environment.interface_declarations[name]
                .declaration
                .type_parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect()
        };
        Substitution::build(&variables, args, None, None, None)
    }

    fn class_substitution(&self, type_: &Type) -> Substitution {
        let (module_type, instance_type) = match type_ {
            Type::Singleton(name) => (type_.clone(), self.factory.instance_type(name)),
            Type::NameInstance { name, .. } => (
                Type::Singleton(name.clone()),
                self.factory.instance_type(name),
            ),
            _ => unreachable!("a class substitution is for a class instance or singleton type"),
        };
        Substitution::build(
            &[],
            &[],
            Some(instance_type),
            Some(module_type),
            Some(type_.clone()),
        )
    }

    fn class_instance_name(type_: &Type) -> &TypeName {
        match type_ {
            Type::NameInstance { name, .. } => name,
            _ => unreachable!("a back type is a class instance type"),
        }
    }

    fn object_shape_with_self(
        &self,
        instance: &Type,
        self_type: Option<Type>,
        shape_type: Type,
    ) -> Result<Shape, RbsError> {
        let substitution = self
            .class_substitution(instance)
            .with_self_type(self_type)
            .merge(&self.app_substitution(instance), false);
        let raw = self.object_shape(Self::class_instance_name(instance))?;
        Ok(Shape::new(Type::Bot, raw).substitute(substitution, Some(shape_type)))
    }

    fn raw_shape(&self, type_: &Type, config: &Config) -> Result<Option<Shape>, RbsError> {
        match type_ {
            Type::SelfType => {
                let self_type = config
                    .self_type
                    .as_ref()
                    .ok_or("the self type is not configured")?;
                self.self_shape(self_type, config)
            }
            Type::Instance => {
                let instance_type = config
                    .instance_type
                    .as_ref()
                    .ok_or("the instance type is not configured")?;
                self.raw_shape(instance_type, config)
            }
            Type::Class => {
                let class_type = config
                    .class_type
                    .as_ref()
                    .ok_or("the class type is not configured")?;
                self.raw_shape(class_type, config)
            }
            Type::Singleton(name) => {
                let raw = self.singleton_shape(name)?;
                Ok(Some(
                    Shape::new(Type::Singleton(name.clone()), raw)
                        .substitute(self.class_substitution(type_), None),
                ))
            }
            Type::NameInstance { name, .. } => {
                let raw = self.object_shape(name)?;
                let substitution = self
                    .class_substitution(type_)
                    .merge(&self.app_substitution(type_), false);
                Ok(Some(
                    Shape::new(Type::Bot, raw).substitute(substitution, Some(type_.clone())),
                ))
            }
            Type::Interface { name, .. } => {
                let raw = self.object_shape(name)?;
                let substitution = Substitution::build(&[], &[], None, None, Some(type_.clone()))
                    .merge(&self.app_substitution(type_), false);
                Ok(Some(
                    Shape::new(Type::Bot, raw).substitute(substitution, Some(type_.clone())),
                ))
            }
            Type::Union(types) => {
                let mut groups: Vec<(Option<Type>, Vec<Type>)> = Vec::new();
                for member in types {
                    let key = match member {
                        Type::Literal(literal) => Some(literal_back_type(literal)),
                        _ => None,
                    };
                    match groups.iter_mut().find(|(existing, _)| *existing == key) {
                        Some((_, members)) => members.push(member.clone()),
                        None => groups.push((key, vec![member.clone()])),
                    }
                }
                let mut shapes: Vec<Shape> = Vec::new();
                for (key, members) in groups {
                    match key {
                        Some(back_type) => {
                            let union = Type::union(members);
                            shapes.push(self.object_shape_with_self(
                                &back_type,
                                Some(union.clone()),
                                union,
                            )?);
                        }
                        None => {
                            for member in members {
                                let Some(shape) = self.raw_shape(&member, config)? else {
                                    return Ok(None);
                                };
                                shapes.push(shape);
                            }
                        }
                    }
                }
                if let Some(shape) = self.union_shape_cache.borrow().get(type_) {
                    return Ok(Some(shape.clone()));
                }
                let shape = self.union_shape(type_, &shapes)?;
                self.union_shape_cache
                    .borrow_mut()
                    .insert(type_.clone(), shape.clone());
                Ok(Some(shape))
            }
            Type::Intersection(types) => {
                let mut raw = RawShape::default();
                for member in types {
                    let Some(shape) = self.raw_shape(member, config)? else {
                        return Ok(None);
                    };
                    for entry in shape.methods(self)? {
                        let keep_old = raw
                            .get(self, &entry.method_name)
                            .is_some_and(|old| !old.private_method && entry.private_method);
                        if !keep_old {
                            raw.insert(RawEntry::resolved(entry));
                        }
                    }
                }
                Ok(Some(Shape::new(type_.clone(), Rc::new(raw))))
            }
            Type::Alias { name, args } => {
                let expanded = self.factory.expand_alias(name, args)?;
                Ok(self.raw_shape(&expanded, config)?.map(|shape| Shape {
                    type_: type_.clone(),
                    ..shape
                }))
            }
            Type::Literal(literal) => {
                let back_type = literal_back_type(literal);
                Ok(Some(self.object_shape_with_self(
                    &back_type,
                    Some(type_.clone()),
                    type_.clone(),
                )?))
            }
            Type::Proc { .. } => Ok(Some(self.proc_shape(type_, Some(type_.clone()))?)),
            Type::Tuple(types) => self.tuple_shape(type_, types).map(Some),
            Type::Record(_) => self.record_shape(type_).map(Some),
            Type::Var(variable) => {
                let Some(bound) = config.variable_bounds.get(variable) else {
                    return Ok(None);
                };
                let bound_config = Config {
                    self_type: Some(bound.clone()),
                    class_type: None,
                    instance_type: None,
                    variable_bounds: config.variable_bounds.clone(),
                };
                let substitution = Substitution::build(&[], &[], None, None, Some(type_.clone()));
                Ok(self
                    .self_shape(bound, &bound_config)?
                    .map(|shape| shape.substitute(substitution, Some(type_.clone()))))
            }
            Type::Nil => {
                let nil_class = instance_type("::NilClass", Vec::new());
                Ok(Some(self.object_shape_with_self(
                    &nil_class,
                    Some(Type::Nil),
                    Type::Nil,
                )?))
            }
            Type::Boolean | Type::Logic(_) => {
                Ok(Some(self.boolean_shape(type_, Some(type_.clone()))?))
            }
            Type::Any | Type::Top | Type::Bot | Type::Void => Ok(None),
        }
    }

    fn self_shape(&self, type_: &Type, config: &Config) -> Result<Option<Shape>, RbsError> {
        match type_ {
            Type::SelfType | Type::Instance | Type::Class => {
                Err(format!("the self shape of `{type_}` is undefined").into())
            }
            Type::Singleton(name) => {
                let raw = self.singleton_shape(name)?;
                let substitution = self.class_substitution(type_).with_self_type(None);
                Ok(Some(
                    Shape::new(type_.clone(), raw).substitute(substitution, None),
                ))
            }
            Type::NameInstance { .. } => Ok(Some(self.object_shape_with_self(
                type_,
                None,
                type_.clone(),
            )?)),
            Type::Interface { name, .. } => {
                let raw = self.object_shape(name)?;
                Ok(Some(Shape::new(Type::Bot, raw).substitute(
                    self.app_substitution(type_),
                    Some(type_.clone()),
                )))
            }
            Type::Literal(literal) => {
                let back_type = literal_back_type(literal);
                Ok(Some(self.object_shape_with_self(
                    &back_type,
                    None,
                    type_.clone(),
                )?))
            }
            Type::Var(variable) => match config.variable_bounds.get(variable) {
                Some(bound) => Ok(self.self_shape(bound, config)?.map(|shape| Shape {
                    type_: type_.clone(),
                    ..shape
                })),
                None => Ok(None),
            },
            Type::Boolean => Ok(Some(self.boolean_shape(type_, None)?)),
            Type::Proc { .. } => Ok(Some(self.proc_shape(type_, None)?)),
            _ => self.raw_shape(type_, config),
        }
    }

    fn proc_shape(&self, type_: &Type, self_type: Option<Type>) -> Result<Shape, RbsError> {
        let Type::Proc {
            function, block, ..
        } = type_
        else {
            unreachable!("a proc shape is for a proc type");
        };
        let proc_instance = instance_type("::Proc", Vec::new());
        let proc_shape = self.object_shape_with_self(&proc_instance, self_type, type_.clone())?;
        let mut raw = proc_shape.resolved(self)?;
        let call = MethodType {
            type_params: Vec::new(),
            type_: (**function).clone(),
            block: block.as_deref().cloned(),
        };
        raw.insert(RawEntry::public("[]", vec![Overload::new(call.clone())]));
        raw.insert(RawEntry::public("call", vec![Overload::new(call)]));
        Ok(Shape::new(type_.clone(), Rc::new(raw)))
    }

    fn boolean_shape(&self, type_: &Type, self_type: Option<Type>) -> Result<Shape, RbsError> {
        let shapes = [
            self.object_shape_with_self(
                &instance_type("::TrueClass", Vec::new()),
                self_type.clone(),
                type_.clone(),
            )?,
            self.object_shape_with_self(
                &instance_type("::FalseClass", Vec::new()),
                self_type,
                type_.clone(),
            )?,
        ];
        self.union_shape(type_, &shapes)
    }

    fn union_shape(&self, type_: &Type, shapes: &[Shape]) -> Result<Shape, RbsError> {
        let member_methods: Vec<Vec<(String, bool, MemberOverloads)>> = shapes
            .iter()
            .map(|shape| shape.member_methods(self))
            .collect::<Result<_, RbsError>>()?;
        let indexes: Vec<HashMap<&str, usize>> = member_methods
            .iter()
            .map(|methods| {
                methods
                    .iter()
                    .enumerate()
                    .map(|(index, (method_name, _, _))| (method_name.as_str(), index))
                    .collect()
            })
            .collect();
        let mut raw = RawShape::default();
        for (method_name, _, _) in member_methods.last().expect("a union has members") {
            let Some(entries) = member_methods
                .iter()
                .zip(&indexes)
                .map(|(methods, indexes)| {
                    indexes
                        .get(method_name.as_str())
                        .map(|index| &methods[*index])
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            raw.insert(RawEntry {
                method_name: method_name.clone(),
                private_method: entries.iter().any(|(_, private_method, _)| *private_method),
                overloads: Rc::new(RefCell::new(LazyOverloads::Union(
                    entries
                        .into_iter()
                        .map(|(_, _, member)| member.clone())
                        .collect(),
                ))),
            });
        }
        Ok(Shape::new(type_.clone(), Rc::new(raw)))
    }

    fn union_overloads(
        &self,
        member_overloads: &[Vec<Overload>],
    ) -> Result<Option<Vec<Overload>>, RbsError> {
        let mut accumulated = member_overloads[0].clone();
        for next in &member_overloads[1..] {
            let same_types = accumulated.len() == next.len()
                && accumulated
                    .iter()
                    .zip(next)
                    .all(|(left, right)| left.method_type == right.method_type);
            if same_types {
                let left_definitions: Vec<Rc<TypeDefinition>> = accumulated
                    .iter()
                    .flat_map(|overload| overload.definitions.clone())
                    .collect();
                let right_definitions: Vec<Rc<TypeDefinition>> = next
                    .iter()
                    .flat_map(|overload| overload.definitions.clone())
                    .collect();
                if same_definitions(&left_definitions, &right_definitions) {
                    continue;
                }
            }
            let mut merged: Vec<Overload> = Vec::new();
            for left in &accumulated {
                for right in next {
                    let definitions = combined_definitions(&left.definitions, &right.definitions);
                    if left.method_type == right.method_type {
                        merged.push(Overload {
                            method_type: left.method_type.clone(),
                            definitions,
                        });
                    } else if let Some(method_type) =
                        MethodType::union(&left.method_type, &right.method_type, &self.subtyping())?
                    {
                        merged.push(Overload {
                            method_type,
                            definitions,
                        });
                    }
                }
            }
            if merged.is_empty() {
                return Ok(None);
            }
            accumulated = merged;
        }
        Ok(Some(accumulated))
    }

    fn tuple_shape(&self, tuple: &Type, types: &[Type]) -> Result<Shape, RbsError> {
        let array = instance_type("::Array", vec![Type::union(types.to_vec())]);
        let array_shape =
            self.object_shape_with_self(&array, Some(tuple.clone()), tuple.clone())?;
        let mut raw = array_shape.resolved(self)?;
        let existing = |method_name: &str| -> Result<Vec<Overload>, RbsError> {
            array_shape
                .method(self, method_name)?
                .map(|entry| entry.overloads)
                .ok_or_else(|| format!("::Array has no `{method_name}`").into())
        };
        let reference: Vec<Overload> = types
            .iter()
            .enumerate()
            .map(|(index, element)| {
                method_type(required(vec![index_literal(index)]), element.clone())
            })
            .map(Overload::new)
            .chain(existing("[]")?)
            .collect();
        let update: Vec<Overload> = types
            .iter()
            .enumerate()
            .map(|(index, element)| {
                method_type(
                    required(vec![index_literal(index), element.clone()]),
                    element.clone(),
                )
            })
            .map(Overload::new)
            .chain(existing("[]=")?)
            .collect();
        let fetch: Vec<Overload> = types
            .iter()
            .enumerate()
            .flat_map(|(index, element)| {
                self.fetch_overloads(
                    index_literal(index),
                    element,
                    instance_type("::Integer", Vec::new()),
                )
            })
            .map(Overload::new)
            .chain(existing("fetch")?)
            .collect();
        raw.insert(RawEntry::public("[]", reference));
        raw.insert(RawEntry::public("[]=", update));
        raw.insert(RawEntry::public("fetch", fetch));
        raw.insert(RawEntry::public(
            "first",
            vec![Overload::new(method_type(
                required(Vec::new()),
                types.first().cloned().unwrap_or(Type::Nil),
            ))],
        ));
        raw.insert(RawEntry::public(
            "last",
            vec![Overload::new(method_type(
                required(Vec::new()),
                types.last().cloned().unwrap_or(Type::Nil),
            ))],
        ));
        if self.steep_version >= SteepVersion::Version2_1_0 {
            raw.insert(RawEntry::public(
                "to_ary",
                vec![Overload::new(method_type(
                    required(Vec::new()),
                    tuple.clone(),
                ))],
            ));
        }
        Ok(Shape::new(tuple.clone(), Rc::new(raw)))
    }

    fn fetch_overloads(&self, key: Type, value: &Type, block_parameter: Type) -> Vec<MethodType> {
        vec![
            method_type(required(vec![key.clone()]), value.clone()),
            MethodType {
                type_params: vec![type_parameter_t()],
                ..method_type(
                    required(vec![key.clone(), variable_t()]),
                    Type::union(vec![value.clone(), variable_t()]),
                )
            },
            MethodType {
                type_params: vec![type_parameter_t()],
                type_: Function {
                    params: Some(required(vec![key])),
                    return_type: Type::union(vec![value.clone(), variable_t()]),
                },
                block: Some(Block {
                    type_: Function {
                        params: Some(required(vec![block_parameter])),
                        return_type: variable_t(),
                    },
                    optional: false,
                    self_type: None,
                }),
            },
        ]
    }

    fn record_shape(&self, record: &Type) -> Result<Shape, RbsError> {
        let Type::Record(elements) = record else {
            unreachable!("a record shape is for a record type");
        };
        let all_key_type = Type::union(
            elements
                .iter()
                .map(|element| literal_back_type(&element.key))
                .collect(),
        );
        let all_value_type = Type::union(
            elements
                .iter()
                .map(|element| element.type_.clone())
                .collect(),
        );
        let hash = instance_type("::Hash", vec![all_key_type.clone(), all_value_type]);
        let hash_shape =
            self.object_shape_with_self(&hash, Some(record.clone()), record.clone())?;
        let mut raw = hash_shape.resolved(self)?;
        let existing = |method_name: &str| -> Result<Vec<Overload>, RbsError> {
            hash_shape
                .method(self, method_name)?
                .map(|entry| entry.overloads)
                .ok_or_else(|| format!("::Hash has no `{method_name}`").into())
        };
        let reference: Vec<Overload> = elements
            .iter()
            .map(|element| {
                let value_type = if element.required {
                    element.type_.clone()
                } else {
                    Type::union(vec![element.type_.clone(), Type::Nil])
                };
                method_type(
                    required(vec![Type::Literal(element.key.clone())]),
                    value_type,
                )
            })
            .map(Overload::new)
            .chain(existing("[]")?)
            .collect();
        let update: Vec<Overload> = elements
            .iter()
            .map(|element| {
                method_type(
                    required(vec![
                        Type::Literal(element.key.clone()),
                        element.type_.clone(),
                    ]),
                    element.type_.clone(),
                )
            })
            .map(Overload::new)
            .chain(existing("[]=")?)
            .collect();
        let fetch: Vec<Overload> = elements
            .iter()
            .flat_map(|element| {
                self.fetch_overloads(
                    Type::Literal(element.key.clone()),
                    &element.type_,
                    all_key_type.clone(),
                )
            })
            .map(Overload::new)
            .chain(existing("fetch")?)
            .collect();
        raw.insert(RawEntry::public("[]", reference));
        raw.insert(RawEntry::public("[]=", update));
        raw.insert(RawEntry::public("fetch", fetch));
        Ok(Shape::new(record.clone(), Rc::new(raw)))
    }

    pub fn shape(&self, type_: &Type, config: &Config) -> Result<Option<Shape>, RbsError> {
        let Some(shape) = self.raw_shape(type_, config)? else {
            return Ok(None);
        };
        if contains_self(type_) {
            return Ok(Some(shape));
        }
        Ok(Some(match config.substitution() {
            Some(substitution) => shape.substitute(substitution, None),
            None => shape,
        }))
    }
}
