use std::{path::Path, rc::Rc};

use rbs_definition::ast::{Declaration, TypeName};

use crate::{
    rbs_locator::{Component, Locator},
    services::{
        completion_provider::type_name::{Prefix, TypeNameCompletion},
        signature_service::{SignatureFile, SignatureStatus},
        type_check_service::TypeCheckService,
    },
    type_inference::constant_env::{Nesting, ResolverContext},
};

pub struct RbsCompletion {
    pub prefix_size: usize,
    pub type_names: Vec<(TypeName, TypeName)>,
}

pub fn run(
    service: &TypeCheckService,
    target_name: &str,
    path: &Path,
    position: usize,
) -> Option<Result<RbsCompletion, String>> {
    let signature = service.signature_services.get(target_name)?;
    let environment = signature.latest_environment()?;
    let mut context: ResolverContext = None;
    let directives = match signature.status {
        SignatureStatus::SyntaxError(_) | SignatureStatus::AncestorError(_) => {
            let absolute_path = service.project.absolute_path(path);
            environment
                .sources
                .iter()
                .find(|source| source.path == absolute_path)
                .map(|source| source.directives.clone())
                .unwrap_or_default()
        }
        SignatureStatus::Loaded => {
            let Some(SignatureFile::Rbs {
                source: Ok(source), ..
            }) = signature.files.get(path)
            else {
                return None;
            };
            let locator = Locator::new(&source.directives, &source.declarations);
            let (_, components) = locator.find(position);
            for component in components.iter().rev() {
                let name = match component {
                    Component::Declaration(Declaration::Class(class)) => &class.name,
                    Component::Declaration(Declaration::Module(module)) => &module.name,
                    _ => continue,
                };
                let last = context.as_ref().and_then(|nesting| nesting.name.clone());
                let name = match last {
                    Some(last) => last.join(name),
                    None => name.absolute(),
                };
                context = Some(Rc::new(Nesting {
                    parent: context,
                    name: Some(name),
                }));
            }
            source.directives.clone()
        }
    };
    let content = signature.files.get(path)?.content();
    let prefix = Prefix::parse(content.get(..position)?);
    let completion = TypeNameCompletion::new(environment, context, &directives);
    let type_names = match completion.find_type_names(prefix.as_ref()) {
        Ok(type_names) => type_names
            .iter()
            .filter_map(|type_name| completion.resolve_name_in_context(type_name))
            .collect(),
        Err(error) => return Some(Err(error)),
    };
    Some(Ok(RbsCompletion {
        prefix_size: prefix.as_ref().map_or(0, Prefix::size),
        type_names,
    }))
}
