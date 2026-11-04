use std::rc::Rc;

use rbs_definition::ast::ClassOrModule;
use steep_interface::types::Type;

use crate::{
    diagnostic::ruby as diagnostics,
    ruby::node::Node,
    ruby_exception,
    strict::StrictCheck,
    type_construction::{Pair, TypeConstruction, instance_type, or_raise},
    type_inference::{
        context::{CallContext, Context, ModuleContext, TypeVariableContext},
        type_env::TypeEnv,
    },
};
use steep_interface::steep_version::SteepVersion;

impl<'checker> TypeConstruction<'checker> {
    pub fn meta_type(&self, type_: &Type) -> Option<Type> {
        match type_ {
            Type::NameInstance { name, .. } => Some(Type::Singleton(name.clone())),
            Type::Singleton(_) => Some(instance_type("::Class", Vec::new())),
            _ => None,
        }
    }

    pub fn for_sclass(&self, node: &Rc<Node>, type_: &Type) -> Option<TypeConstruction<'checker>> {
        let annotations = self.annotations_for(Some(node), &self.nesting());
        let instance_type = match type_ {
            Type::SelfType => self.self_type().clone(),
            _ => type_.clone(),
        };
        let module_type = match &instance_type {
            Type::Singleton(type_name) => {
                let environment = self.checker.environment();
                let normalized = environment.normalize_module_name(type_name);
                match environment.class_declarations.get(&normalized) {
                    Some(entry) if entry.kind == ClassOrModule::Module => {
                        crate::type_construction::instance_type("::Module", Vec::new())
                    }
                    Some(_) => crate::type_construction::instance_type("::Class", Vec::new()),
                    None => ruby_exception::raise_runtime_error(""),
                }
            }
            Type::NameInstance { name, .. } => Type::Singleton(name.clone()),
            _ => return None,
        };
        let definition_builder = &self.builder().definition_builder;
        let instance_definition = match &instance_type {
            Type::Singleton(type_name) => or_raise(definition_builder.build_singleton(type_name)),
            Type::NameInstance { name, .. } => or_raise(definition_builder.build_instance(name)),
            _ => return None,
        };
        let module_definition = match &module_type {
            Type::Singleton(type_name) => {
                Some(or_raise(definition_builder.build_singleton(type_name)))
            }
            _ => None,
        };
        let module_context = ModuleContext {
            instance_type: annotations.instance_type.clone().unwrap_or(instance_type),
            module_type: annotations
                .self_type
                .clone()
                .or_else(|| annotations.module_type.clone())
                .unwrap_or(module_type),
            implement_name: None,
            nesting: self.nesting(),
            class_name: self.module_context().class_name.clone(),
            instance_definition: Some(instance_definition.clone()),
            module_definition,
            defined_instance_methods: Rc::default(),
            defined_module_methods: Rc::default(),
        };
        if self.steep_version() == SteepVersion::Version2_0_0 {
            or_raise(definition_builder.build_singleton(&module_context.class_name));
        }
        let mut type_env = TypeEnv::new(self.context.type_env.constant_env.clone());
        self.import_globals(&mut type_env);
        for (name, type_) in &annotations.const_types {
            type_env.constant_types.insert(name.clone(), type_.clone());
        }
        let mut type_env = self.type_env_with_local_annotations(&type_env, &annotations);
        self.import_instance_variables(&mut type_env, Some(&instance_definition));
        let self_type = self
            .meta_type(annotations.self_type.as_ref().unwrap_or(type_))
            .unwrap_or_else(|| Type::Singleton(rbs_definition::ast::TypeName::parse("::Class")));
        let class_name = module_context.class_name.clone();
        Some(TypeConstruction {
            checker: self.checker,
            source: self.source,
            annotations: Rc::new(annotations),
            typing: self.typing.clone(),
            context: Context {
                method_context: None,
                block_context: None,
                break_context: None,
                module_context: Rc::new(module_context),
                self_type,
                type_env,
                call_context: CallContext::Module(class_name),
                variable_context: Rc::new(TypeVariableContext::empty()),
            },
        })
    }

    pub fn synthesize_singleton_class(&self, node: &Rc<Node>) -> Pair<'checker> {
        let receiver = node.child_node(0).cloned().expect("sclass has a receiver");
        let pair = self.synthesize(&receiver, None, false);
        let (type_, constr) = (pair.type_, pair.constr);
        let Some(constructor) = self.for_sclass(node, &type_) else {
            self.add_error(diagnostics::unsupported_syntax(
                node,
                Some(&format!(
                    "sclass receiver must be instance type or singleton type, but type given `{type_}`"
                )),
            ));
            self.observe(
                StrictCheck::SingletonClassReceiver,
                node,
                format!("Steep cannot open the singleton class of `{type_}`, and does not type check the body"),
            );
            return constr.add_typing(node, Type::Nil);
        };
        constructor.with_variable_bounds(|constructor| {
            if let Some(body) = node.child_node(1) {
                constructor.synthesize(body, None, false);
            }
            if let (Some(instance_definition), Some(module_definition)) = (
                &constructor.module_context().instance_definition,
                &self.module_context().module_definition,
            ) && instance_definition.type_name == module_definition.type_name
            {
                let defined_instance_methods = constructor
                    .module_context()
                    .defined_instance_methods
                    .borrow()
                    .clone();
                let mut defined_module_methods =
                    self.module_context().defined_module_methods.borrow_mut();
                for name in defined_instance_methods {
                    if !defined_module_methods.contains(&name) {
                        defined_module_methods.push(name);
                    }
                }
            }
        });
        constr.add_typing(node, Type::Nil)
    }
}
