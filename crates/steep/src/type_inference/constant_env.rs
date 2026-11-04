use std::{cell::RefCell, rc::Rc, sync::Arc};

use rbs_definition::{
    ancestor_builder::Ancestor,
    ast::{ClassOrModule, Namespace, TypeName},
    collections::{HashMap, IndexMap},
};
use steep_interface::{builder::Builder, types::Type};

#[derive(Clone, Debug, PartialEq)]
pub enum ConstantEntry {
    Class(ClassOrModule),
    ClassAlias,
    Constant,
}

#[derive(Clone, Debug)]
pub struct Constant {
    pub name: TypeName,
    pub type_: Type,
    pub entry: ConstantEntry,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Nesting {
    pub parent: Option<Rc<Nesting>>,
    pub name: Option<TypeName>,
}

pub type ResolverContext = Option<Rc<Nesting>>;

pub fn nesting_names(context: &ResolverContext) -> Vec<TypeName> {
    let mut names = Vec::new();
    let mut current = context.clone();
    while let Some(nesting) = current {
        if let Some(name) = &nesting.name {
            names.push(name.clone());
        }
        current = nesting.parent.clone();
    }
    names.reverse();
    names
}

type ConstantTable = IndexMap<String, Constant>;

type ContextKey = Option<Vec<Option<TypeName>>>;

// Why not merge the tables into one table per context as Steep does: every context
// would copy the thousands of toplevel constants. Looking the tables up from the
// last merged one finds the same constant as the merged table does.
enum ConstantSource {
    Children(TypeName),
    Toplevel,
    Itself(String, Constant),
}

pub struct ConstantTables {
    children_table: HashMap<TypeName, ConstantTable>,
    toplevel: ConstantTable,
}

pub struct ConstantResolver {
    tables: Arc<ConstantTables>,
    context_sources_cache: RefCell<HashMap<ContextKey, Option<Rc<Vec<ConstantSource>>>>>,
    child_sources_cache: RefCell<HashMap<TypeName, Rc<Vec<TypeName>>>>,
}

fn object_name() -> TypeName {
    TypeName::parse("::Object")
}

fn context_key(context: &ResolverContext) -> ContextKey {
    let mut names = Vec::new();
    let mut current = context.clone();
    current.as_ref()?;
    while let Some(nesting) = current {
        names.push(nesting.name.clone());
        current = nesting.parent.clone();
    }
    Some(names)
}

impl ConstantTables {
    pub fn new(builder: &Builder) -> ConstantTables {
        let environment = builder.definition_builder.ancestor_builder.environment();
        let mut children_table: HashMap<TypeName, ConstantTable> = HashMap::default();
        let mut toplevel = ConstantTable::default();
        let mut class_names: Vec<&TypeName> = environment.class_declarations.keys().collect();
        class_names.sort_by_key(|name| name.to_string());
        for name in &class_names {
            children_table.insert((*name).clone(), ConstantTable::default());
        }
        for name in &class_names {
            let entry = &environment.class_declarations[*name];
            let constant = Constant {
                name: (*name).clone(),
                type_: Type::Singleton((*name).clone()),
                entry: ConstantEntry::Class(entry.kind),
            };
            if name.namespace.is_empty() {
                toplevel.insert(name.name.clone(), constant);
            } else if let Some(table) = children_table.get_mut(&name.namespace.to_type_name()) {
                table.insert(name.name.clone(), constant);
            }
        }
        let mut alias_names: Vec<&TypeName> = environment.class_alias_declarations.keys().collect();
        alias_names.sort_by_key(|name| name.to_string());
        for name in alias_names {
            let normalized = environment.normalize_module_name(name);
            if !environment.class_declarations.contains_key(&normalized) {
                continue;
            }
            let constant = Constant {
                name: name.clone(),
                type_: Type::Singleton(name.clone()),
                entry: ConstantEntry::ClassAlias,
            };
            if name.namespace.is_empty() {
                toplevel.insert(name.name.clone(), constant);
            } else {
                let parent = environment.normalize_module_name(&name.namespace.to_type_name());
                if let Some(table) = children_table.get_mut(&parent) {
                    table.insert(name.name.clone(), constant);
                }
            }
        }
        let mut constant_names: Vec<&TypeName> = environment.constant_declarations.keys().collect();
        constant_names.sort_by_key(|name| name.to_string());
        for name in constant_names {
            let entry = &environment.constant_declarations[name];
            let constant = Constant {
                name: name.clone(),
                type_: builder.factory.type_(&entry.declaration.type_),
                entry: ConstantEntry::Constant,
            };
            if name.namespace.is_empty() {
                toplevel.insert(name.name.clone(), constant);
            } else if let Some(table) = children_table.get_mut(&name.namespace.to_type_name()) {
                table.insert(name.name.clone(), constant);
            }
        }
        ConstantTables {
            children_table,
            toplevel,
        }
    }
}

impl ConstantResolver {
    pub fn new(tables: Arc<ConstantTables>) -> ConstantResolver {
        ConstantResolver {
            tables,
            context_sources_cache: RefCell::default(),
            child_sources_cache: RefCell::default(),
        }
    }

    pub fn toplevel(&self, name: &str) -> Option<Constant> {
        self.tables.toplevel.get(name).cloned()
    }

    pub fn children_table(&self, name: &TypeName) -> Option<&ConstantTable> {
        self.tables.children_table.get(name)
    }

    pub fn resolve(
        &self,
        builder: &Builder,
        name: &str,
        context: &ResolverContext,
    ) -> Option<Constant> {
        let sources = self.context_sources(builder, context)?;
        sources
            .iter()
            .rev()
            .find_map(|source| match source {
                ConstantSource::Children(module_name) => {
                    self.tables.children_table[module_name].get(name)
                }
                ConstantSource::Toplevel => self.tables.toplevel.get(name),
                ConstantSource::Itself(constant_name, constant) => {
                    (constant_name == name).then_some(constant)
                }
            })
            .cloned()
    }

    fn context_sources(
        &self,
        builder: &Builder,
        context: &ResolverContext,
    ) -> Option<Rc<Vec<ConstantSource>>> {
        let key = context_key(context);
        if let Some(cached) = self.context_sources_cache.borrow().get(&key) {
            return cached.clone();
        }
        let sources = self.load_context_sources(builder, context).map(Rc::new);
        self.context_sources_cache
            .borrow_mut()
            .insert(key, sources.clone());
        sources
    }

    pub fn resolve_child(
        &self,
        builder: &Builder,
        module_name: &TypeName,
        name: &str,
    ) -> Option<Constant> {
        self.child_sources(builder, module_name)
            .iter()
            .rev()
            .find_map(|ancestor| self.tables.children_table[ancestor].get(name))
            .cloned()
    }

    fn child_sources(&self, builder: &Builder, module_name: &TypeName) -> Rc<Vec<TypeName>> {
        let environment = builder.definition_builder.ancestor_builder.environment();
        let module_name = environment.normalize_module_name(module_name);
        if let Some(cached) = self.child_sources_cache.borrow().get(&module_name) {
            return cached.clone();
        }
        let sources = Rc::new(self.load_child_sources(builder, &module_name));
        self.child_sources_cache
            .borrow_mut()
            .insert(module_name, sources.clone());
        sources
    }

    fn included_ancestors(builder: &Builder, name: &TypeName) -> Vec<TypeName> {
        let Ok(ancestors) = builder
            .definition_builder
            .ancestor_builder
            .instance_ancestors(name)
        else {
            return Vec::new();
        };
        let mut prepended: Vec<TypeName> = Vec::new();
        for ancestor in &ancestors.ancestors {
            if let Ancestor::Instance { name, .. } = ancestor
                && let Ok(one) = builder
                    .definition_builder
                    .ancestor_builder
                    .one_instance_ancestors(name)
                && let Some(modules) = &one.prepended_modules
            {
                prepended.extend(modules.iter().map(|module| module.name().clone()));
            }
        }
        ancestors
            .ancestors
            .iter()
            .rev()
            .filter_map(|ancestor| match ancestor {
                Ancestor::Instance { name, .. } if !prepended.contains(name) => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    fn load_child_sources(&self, builder: &Builder, name: &TypeName) -> Vec<TypeName> {
        let mut sources = Vec::new();
        if self.tables.children_table.contains_key(name) {
            for ancestor in Self::included_ancestors(builder, name) {
                if ancestor == object_name() && *name != object_name() {
                    continue;
                }
                if self.tables.children_table.contains_key(&ancestor) {
                    sources.push(ancestor);
                }
            }
        }
        sources
    }

    fn load_context_sources(
        &self,
        builder: &Builder,
        context: &ResolverContext,
    ) -> Option<Vec<ConstantSource>> {
        let mut sources = Vec::new();
        let last = context.as_ref().and_then(|nesting| nesting.name.clone());
        self.sources_from_ancestors(builder, &last.unwrap_or_else(object_name), &mut sources);
        if !self.sources_from_context(builder, context, &mut sources) {
            return None;
        }
        self.source_itself(context, &mut sources);
        Some(sources)
    }

    fn sources_from_context(
        &self,
        builder: &Builder,
        context: &ResolverContext,
        sources: &mut Vec<ConstantSource>,
    ) -> bool {
        let Some(nesting) = context else {
            return true;
        };
        if !self.sources_from_context(builder, &nesting.parent, sources) {
            return false;
        }
        if let Some(last) = &nesting.name {
            let environment = builder.definition_builder.ancestor_builder.environment();
            let normalized = environment.normalize_module_name(last);
            if !self.tables.children_table.contains_key(&normalized) {
                return false;
            }
            sources.push(ConstantSource::Children(normalized));
        }
        true
    }

    fn sources_from_ancestors(
        &self,
        builder: &Builder,
        module_name: &TypeName,
        sources: &mut Vec<ConstantSource>,
    ) {
        let environment = builder.definition_builder.ancestor_builder.environment();
        let module_name = environment.normalize_module_name(module_name);
        let Some(entry) = environment.class_declarations.get(&module_name) else {
            return;
        };
        if self.tables.children_table.contains_key(&object_name()) {
            sources.push(ConstantSource::Children(object_name()));
        }
        sources.push(ConstantSource::Toplevel);
        for ancestor in Self::included_ancestors(builder, &module_name) {
            if !self.tables.children_table.contains_key(&ancestor) {
                continue;
            }
            let adds_toplevel = ancestor == object_name() && entry.kind == ClassOrModule::Class;
            sources.push(ConstantSource::Children(ancestor));
            if adds_toplevel {
                sources.push(ConstantSource::Toplevel);
            }
        }
    }

    fn source_itself(&self, context: &ResolverContext, sources: &mut Vec<ConstantSource>) {
        let Some(nesting) = context else {
            return;
        };
        let Some(type_name) = &nesting.name else {
            return;
        };
        let constant = if type_name.namespace.is_empty() {
            self.tables.toplevel.get(&type_name.name).cloned()
        } else {
            self.tables
                .children_table
                .get(&type_name.namespace.to_type_name())
                .and_then(|table| table.get(&type_name.name).cloned())
        };
        if let Some(constant) = constant {
            sources.push(ConstantSource::Itself(type_name.name.clone(), constant));
        }
    }
}

pub struct ConstantEnv {
    pub context: ResolverContext,
    pub resolver: Rc<ConstantResolver>,
}

impl ConstantEnv {
    pub fn resolve(&self, builder: &Builder, name: &str) -> Option<Constant> {
        self.resolver.resolve(builder, name, &self.context)
    }

    pub fn toplevel(&self, name: &str) -> Option<Constant> {
        self.resolver.toplevel(name)
    }

    pub fn resolve_child(
        &self,
        builder: &Builder,
        module_name: &TypeName,
        name: &str,
    ) -> Option<Constant> {
        self.resolver.resolve_child(builder, module_name, name)
    }
}

pub fn root_namespace() -> Namespace {
    Namespace::root()
}
