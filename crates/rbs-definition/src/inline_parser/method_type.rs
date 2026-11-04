use std::sync::Arc;

use crate::{
    ast::{Block, Callable, Function, MethodType, Overload, Parameter, Type},
    inline_parser::{
        annotation::{Annotation, ByteRange},
        comment_block::{LocatedAnnotation, Paragraph, TrailingAnnotation},
    },
    location::{AstLocation, Location},
};

pub enum DefinitionParameter {
    Required(String),
    Optional(String),
    Rest(Option<String>),
    Trailing(String),
    RequiredKeyword(String),
    OptionalKeyword(String),
    RestKeyword(Option<String>),
    Block(Option<String>),
}

#[derive(Clone, Debug)]
pub enum Unused {
    Annotation(LocatedAnnotation),
    SyntaxError(crate::inline_parser::comment_block::AnnotationSyntaxError),
}

enum ParameterAnnotation<T> {
    Annotated(T),
    Name(Option<String>),
}

struct DocumentStyle {
    return_type: Option<Type>,
    required_positionals: Vec<ParameterAnnotation<(String, Type)>>,
    optional_positionals: Vec<ParameterAnnotation<(String, Type)>>,
    rest_positionals: Option<ParameterAnnotation<(Option<String>, Type)>>,
    trailing_positionals: Vec<ParameterAnnotation<(String, Type)>>,
    required_keywords: Vec<(String, Option<Type>)>,
    optional_keywords: Vec<(String, Option<Type>)>,
    rest_keywords: Option<ParameterAnnotation<(Option<String>, Type)>>,
    block: Option<ParameterAnnotation<Block>>,
}

fn any() -> Type {
    Type::Any
}

impl DocumentStyle {
    fn build(
        parameter_annotations: Vec<LocatedAnnotation>,
        return_type: Option<Type>,
        parameters: Option<&[DefinitionParameter]>,
    ) -> (DocumentStyle, Vec<Unused>) {
        let mut document = DocumentStyle {
            return_type,
            required_positionals: Vec::new(),
            optional_positionals: Vec::new(),
            rest_positionals: None,
            trailing_positionals: Vec::new(),
            required_keywords: Vec::new(),
            optional_keywords: Vec::new(),
            rest_keywords: None,
            block: None,
        };
        let mut splat_annotation: Option<LocatedAnnotation> = None;
        let mut double_splat_annotation: Option<LocatedAnnotation> = None;
        let mut block_annotation: Option<LocatedAnnotation> = None;
        let mut named_annotations: Vec<(String, LocatedAnnotation)> = Vec::new();
        let mut unused = Vec::new();

        for annotation in parameter_annotations {
            match &annotation.annotation {
                Annotation::SplatParameterType { .. } => {
                    if splat_annotation.is_some() {
                        unused.push(Unused::Annotation(annotation));
                    } else {
                        splat_annotation = Some(annotation);
                    }
                }
                Annotation::DoubleSplatParameterType { .. } => {
                    if double_splat_annotation.is_some() {
                        unused.push(Unused::Annotation(annotation));
                    } else {
                        double_splat_annotation = Some(annotation);
                    }
                }
                Annotation::BlockParameterType { .. } => {
                    if block_annotation.is_some() {
                        unused.push(Unused::Annotation(annotation));
                    } else {
                        block_annotation = Some(annotation);
                    }
                }
                Annotation::ParameterType { name, .. } => {
                    if named_annotations
                        .iter()
                        .any(|(existing, _)| existing == name)
                    {
                        unused.push(Unused::Annotation(annotation));
                    } else {
                        named_annotations.push((name.clone(), annotation));
                    }
                }
                _ => {}
            }
        }

        let mut take_named = |name: &str| -> Option<Type> {
            let index = named_annotations
                .iter()
                .position(|(existing, _)| existing == name)?;
            let (_, annotation) = named_annotations.remove(index);
            match annotation.annotation {
                Annotation::ParameterType { parameter_type, .. } => Some(parameter_type),
                _ => unreachable!("only parameter type annotations are named"),
            }
        };

        let has_block_parameter = parameters.is_some_and(|parameters| {
            parameters
                .iter()
                .any(|parameter| matches!(parameter, DefinitionParameter::Block(_)))
        });

        for parameter in parameters.unwrap_or_default() {
            match parameter {
                DefinitionParameter::Required(name) => {
                    document.required_positionals.push(match take_named(name) {
                        Some(type_) => ParameterAnnotation::Annotated((name.clone(), type_)),
                        None => ParameterAnnotation::Name(Some(name.clone())),
                    })
                }
                DefinitionParameter::Optional(name) => {
                    document.optional_positionals.push(match take_named(name) {
                        Some(type_) => ParameterAnnotation::Annotated((name.clone(), type_)),
                        None => ParameterAnnotation::Name(Some(name.clone())),
                    })
                }
                DefinitionParameter::Rest(rest_name) => {
                    let matches = splat_annotation.as_ref().is_some_and(|annotation| {
                        match &annotation.annotation {
                            Annotation::SplatParameterType { name, .. } => {
                                name.is_none() || rest_name.is_none() || name == rest_name
                            }
                            _ => false,
                        }
                    });
                    document.rest_positionals = Some(if matches {
                        match splat_annotation
                            .take()
                            .map(|annotation| annotation.annotation)
                        {
                            Some(Annotation::SplatParameterType {
                                name,
                                parameter_type,
                                ..
                            }) => ParameterAnnotation::Annotated((name, parameter_type)),
                            _ => unreachable!("the splat annotation exists"),
                        }
                    } else {
                        ParameterAnnotation::Name(rest_name.clone())
                    });
                }
                DefinitionParameter::Trailing(name) => {
                    document.trailing_positionals.push(match take_named(name) {
                        Some(type_) => ParameterAnnotation::Annotated((name.clone(), type_)),
                        None => ParameterAnnotation::Name(Some(name.clone())),
                    })
                }
                DefinitionParameter::RequiredKeyword(name) => {
                    let type_ = take_named(name);
                    document.required_keywords.push((name.clone(), type_));
                }
                DefinitionParameter::OptionalKeyword(name) => {
                    let type_ = take_named(name);
                    document.optional_keywords.push((name.clone(), type_));
                }
                DefinitionParameter::RestKeyword(rest_name) => {
                    let matches =
                        double_splat_annotation.as_ref().is_some_and(
                            |annotation| match &annotation.annotation {
                                Annotation::DoubleSplatParameterType { name, .. } => {
                                    name.is_none() || rest_name.is_none() || name == rest_name
                                }
                                _ => false,
                            },
                        );
                    document.rest_keywords = Some(if matches {
                        match double_splat_annotation
                            .take()
                            .map(|annotation| annotation.annotation)
                        {
                            Some(Annotation::DoubleSplatParameterType {
                                name,
                                parameter_type,
                                ..
                            }) => ParameterAnnotation::Annotated((name, parameter_type)),
                            _ => unreachable!("the double splat annotation exists"),
                        }
                    } else {
                        ParameterAnnotation::Name(rest_name.clone())
                    });
                }
                DefinitionParameter::Block(block_name) => {
                    let matches = block_annotation.as_ref().is_some_and(|annotation| {
                        match &annotation.annotation {
                            Annotation::BlockParameterType { name, .. } => {
                                name.is_none() || block_name.is_none() || name == block_name
                            }
                            _ => false,
                        }
                    });
                    document.block = Some(if matches {
                        match block_annotation
                            .take()
                            .map(|annotation| annotation.annotation)
                        {
                            Some(Annotation::BlockParameterType { block, .. }) => {
                                ParameterAnnotation::Annotated(*block)
                            }
                            _ => unreachable!("the block annotation exists"),
                        }
                    } else {
                        ParameterAnnotation::Name(block_name.clone())
                    });
                }
            }
        }

        if !has_block_parameter
            && let Some(annotation) = block_annotation.take()
            && let Annotation::BlockParameterType { block, .. } = annotation.annotation
        {
            document.block = Some(ParameterAnnotation::Annotated(*block));
        }

        unused.extend(
            named_annotations
                .into_iter()
                .map(|(_, annotation)| Unused::Annotation(annotation)),
        );
        unused.extend(splat_annotation.map(Unused::Annotation));
        unused.extend(double_splat_annotation.map(Unused::Annotation));
        unused.extend(block_annotation.map(Unused::Annotation));

        (document, unused)
    }

    fn positional(annotation: &ParameterAnnotation<(String, Type)>) -> Parameter {
        match annotation {
            ParameterAnnotation::Annotated((name, type_)) => Parameter {
                type_: type_.clone(),
                name: Some(name.clone()),
            },
            ParameterAnnotation::Name(name) => Parameter {
                type_: any(),
                name: name.clone(),
            },
        }
    }

    fn rest(annotation: &ParameterAnnotation<(Option<String>, Type)>) -> Parameter {
        match annotation {
            ParameterAnnotation::Annotated((name, type_)) => Parameter {
                type_: type_.clone(),
                name: name.clone(),
            },
            ParameterAnnotation::Name(name) => Parameter {
                type_: any(),
                name: name.clone(),
            },
        }
    }

    fn keyword(name: &str, type_: &Option<Type>) -> (String, Parameter) {
        (
            name.to_string(),
            Parameter {
                type_: type_.clone().unwrap_or_else(any),
                name: None,
            },
        )
    }

    fn method_type(&self) -> MethodType {
        let function = Function {
            required_positionals: self
                .required_positionals
                .iter()
                .map(Self::positional)
                .collect(),
            optional_positionals: self
                .optional_positionals
                .iter()
                .map(Self::positional)
                .collect(),
            rest_positionals: self.rest_positionals.as_ref().map(Self::rest),
            trailing_positionals: self
                .trailing_positionals
                .iter()
                .map(Self::positional)
                .collect(),
            required_keywords: self
                .required_keywords
                .iter()
                .map(|(name, type_)| Self::keyword(name, type_))
                .collect(),
            optional_keywords: self
                .optional_keywords
                .iter()
                .map(|(name, type_)| Self::keyword(name, type_))
                .collect(),
            rest_keywords: self.rest_keywords.as_ref().map(Self::rest),
            return_type: self.return_type.clone().unwrap_or_else(any),
        };
        let block = self.block.as_ref().map(|block| match block {
            ParameterAnnotation::Annotated(block) => block.clone(),
            ParameterAnnotation::Name(_) => Block {
                type_: Callable::Untyped { return_type: any() },
                required: false,
                self_type: None,
            },
        });
        MethodType {
            type_parameters: Vec::new(),
            type_: Callable::Function(Box::new(function)),
            block,
            location: AstLocation::default(),
        }
    }
}

enum TypeAnnotations {
    Overloads(Vec<LocatedAnnotation>),
    Document(Box<DocumentStyle>),
}

pub struct MethodTypeAnnotation {
    type_annotations: Option<TypeAnnotations>,
    return_type_annotation: Option<LocatedAnnotation>,
}

pub struct BuiltMethodTypeAnnotation {
    pub method_type: MethodTypeAnnotation,
    pub unused_leading: Vec<Unused>,
    pub unused_trailing: Option<Unused>,
}

impl MethodTypeAnnotation {
    pub fn build(
        leading_paragraphs: Option<Vec<Paragraph>>,
        trailing_annotation: Option<TrailingAnnotation>,
        parameters: Option<&[DefinitionParameter]>,
    ) -> BuiltMethodTypeAnnotation {
        let mut unused_leading = Vec::new();
        let mut unused_trailing = None;
        let mut overloads: Option<Vec<LocatedAnnotation>> = None;
        let mut return_type: Option<Type> = None;
        let mut return_type_annotation: Option<LocatedAnnotation> = None;
        let mut parameter_annotations = Vec::new();

        match trailing_annotation {
            Some(TrailingAnnotation::Annotation(
                annotation @ LocatedAnnotation {
                    annotation: Annotation::NodeTypeAssertion { .. },
                    ..
                },
            )) => {
                if let Annotation::NodeTypeAssertion { type_, .. } = &annotation.annotation {
                    return_type = Some(type_.clone());
                }
                return_type_annotation = Some(annotation);
            }
            Some(TrailingAnnotation::Annotation(annotation)) => {
                unused_trailing = Some(Unused::Annotation(annotation))
            }
            Some(TrailingAnnotation::SyntaxError(error)) => {
                unused_trailing = Some(Unused::SyntaxError(error))
            }
            None => {}
        }

        for paragraph in leading_paragraphs.unwrap_or_default() {
            let annotation = match paragraph {
                Paragraph::Text => continue,
                Paragraph::SyntaxError(error) => {
                    unused_leading.push(Unused::SyntaxError(error));
                    continue;
                }
                Paragraph::Annotation(annotation) => annotation,
            };
            match &annotation.annotation {
                Annotation::MethodTypes { .. } | Annotation::ColonMethodType { .. } => {
                    overloads.get_or_insert_with(Vec::new).push(annotation);
                    continue;
                }
                Annotation::ReturnType {
                    return_type: type_, ..
                } if overloads.is_none() && return_type.is_none() => {
                    return_type = Some(type_.clone());
                    return_type_annotation = Some(annotation);
                    continue;
                }
                Annotation::ParameterType { .. }
                | Annotation::SplatParameterType { .. }
                | Annotation::DoubleSplatParameterType { .. }
                | Annotation::BlockParameterType { .. }
                    if overloads.is_none() =>
                {
                    parameter_annotations.push(annotation);
                    continue;
                }
                _ => {}
            }
            unused_leading.push(Unused::Annotation(annotation));
        }

        let type_annotations = match overloads {
            Some(overloads) => Some(TypeAnnotations::Overloads(overloads)),
            None if return_type.is_some() || !parameter_annotations.is_empty() => {
                let (document, unused) =
                    DocumentStyle::build(parameter_annotations, return_type, parameters);
                unused_leading.extend(unused);
                Some(TypeAnnotations::Document(Box::new(document)))
            }
            None => None,
        };

        BuiltMethodTypeAnnotation {
            method_type: MethodTypeAnnotation {
                type_annotations,
                return_type_annotation,
            },
            unused_leading,
            unused_trailing,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.type_annotations.is_none()
    }

    pub fn type_regions(&self) -> Vec<(ByteRange, Vec<Option<Location>>)> {
        match &self.type_annotations {
            Some(TypeAnnotations::Overloads(annotations)) => annotations
                .iter()
                .filter_map(|annotation| match &annotation.annotation {
                    Annotation::ColonMethodType { type_locations, .. }
                    | Annotation::MethodTypes { type_locations, .. } => {
                        Some((annotation.location, type_locations.clone()))
                    }
                    _ => None,
                })
                .collect(),
            Some(TypeAnnotations::Document(_)) => self
                .return_type_annotation
                .iter()
                .filter_map(|annotation| match &annotation.annotation {
                    Annotation::ReturnType { type_location, .. }
                    | Annotation::NodeTypeAssertion { type_location, .. } => {
                        Some((annotation.location, vec![type_location.clone()]))
                    }
                    _ => None,
                })
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn overloads(&self) -> Vec<Overload> {
        match &self.type_annotations {
            Some(TypeAnnotations::Document(document)) => vec![Overload {
                method_type: Arc::new(document.method_type()),
                annotations: Vec::new(),
            }],
            Some(TypeAnnotations::Overloads(annotations)) => annotations
                .iter()
                .flat_map(|annotation| match &annotation.annotation {
                    Annotation::ColonMethodType {
                        annotations,
                        method_type,
                        ..
                    } => vec![Overload {
                        method_type: Arc::new((**method_type).clone()),
                        annotations: annotations.clone(),
                    }],
                    Annotation::MethodTypes { overloads, .. } => overloads.clone(),
                    _ => unreachable!("only method type annotations are overloads"),
                })
                .collect(),
            None => vec![Overload {
                method_type: Arc::new(MethodType {
                    type_parameters: Vec::new(),
                    type_: Callable::Untyped { return_type: any() },
                    block: None,
                    location: AstLocation::default(),
                }),
                annotations: Vec::new(),
            }],
        }
    }

    pub fn is_overloading(&self) -> bool {
        match &self.type_annotations {
            Some(TypeAnnotations::Overloads(annotations)) => annotations.iter().any(|annotation| {
                matches!(
                    annotation.annotation,
                    Annotation::MethodTypes {
                        overloading: true,
                        ..
                    }
                )
            }),
            _ => false,
        }
    }
}
