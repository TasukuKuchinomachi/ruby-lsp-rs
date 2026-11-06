use std::fmt;

use crate::{
    builder::{Entry, Shape},
    method_type::{Block, Function, MethodType, Params},
    types::Type,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Relation<Item> {
    pub sub_type: Item,
    pub super_type: Item,
}

impl<Item> Relation<Item> {
    pub fn new(sub_type: Item, super_type: Item) -> Relation<Item> {
        Relation {
            sub_type,
            super_type,
        }
    }

    pub fn flip(self) -> Relation<Item> {
        Relation {
            sub_type: self.super_type,
            super_type: self.sub_type,
        }
    }

    pub fn map<Mapped>(&self, mut transform: impl FnMut(&Item) -> Mapped) -> Relation<Mapped> {
        let sub_type = transform(&self.sub_type);
        let super_type = transform(&self.super_type);
        Relation {
            sub_type,
            super_type,
        }
    }
}

impl<Item: fmt::Display> fmt::Display for Relation<Item> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} <: {}", self.sub_type, self.super_type)
    }
}

#[derive(Clone, Debug)]
pub enum MethodOrEntry {
    Entry(Entry),
    MethodType(Box<MethodType>),
}

impl fmt::Display for MethodOrEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MethodOrEntry::Entry(entry) => entry.fmt(formatter),
            MethodOrEntry::MethodType(method_type) => method_type.fmt(formatter),
        }
    }
}

#[derive(Clone, Debug)]
pub enum AnyRelation {
    Type(Relation<Type>),
    MissingUpperBound(Type),
    Interface(Relation<Shape>),
    Method(Box<Relation<MethodOrEntry>>),
    Function(Box<Relation<Function>>),
    Params(Box<Relation<Params>>),
    Block(Box<Relation<Option<Block>>>),
}

fn optional_block(block: &Option<Block>) -> String {
    block.as_ref().map(ToString::to_string).unwrap_or_default()
}

impl AnyRelation {
    pub fn kind(&self) -> &'static str {
        match self {
            AnyRelation::Type(_) | AnyRelation::MissingUpperBound(_) => "type",
            AnyRelation::Interface(_) => "interface",
            AnyRelation::Method(_) => "method",
            AnyRelation::Function(_) => "function",
            AnyRelation::Params(_) => "params",
            AnyRelation::Block(_) => "block",
        }
    }

    pub fn diagnostic_message(&self) -> Option<String> {
        match self {
            AnyRelation::Type(_) | AnyRelation::MissingUpperBound(_) => Some(self.to_string()),
            AnyRelation::Method(relation) => match &**relation {
                Relation {
                    sub_type: MethodOrEntry::MethodType(_),
                    super_type: MethodOrEntry::MethodType(_),
                } => Some(self.to_string()),
                _ => None,
            },
            AnyRelation::Interface(_) | AnyRelation::Function(_) => None,
            AnyRelation::Block(_) => Some("(Blocks are incompatible)".to_string()),
            AnyRelation::Params(_) => Some("(Params are incompatible)".to_string()),
        }
    }
}

impl fmt::Display for AnyRelation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnyRelation::Type(relation) => relation.fmt(formatter),
            AnyRelation::MissingUpperBound(type_) => write!(formatter, "{type_} <: "),
            AnyRelation::Interface(relation) => write!(
                formatter,
                "{} <: {}",
                relation.sub_type.type_, relation.super_type.type_
            ),
            AnyRelation::Method(relation) => relation.fmt(formatter),
            AnyRelation::Function(relation) => relation.fmt(formatter),
            AnyRelation::Params(relation) => relation.fmt(formatter),
            AnyRelation::Block(relation) => write!(
                formatter,
                "{} <: {}",
                optional_block(&relation.sub_type),
                optional_block(&relation.super_type)
            ),
        }
    }
}

impl From<Relation<Type>> for AnyRelation {
    fn from(relation: Relation<Type>) -> AnyRelation {
        AnyRelation::Type(relation)
    }
}

impl From<Relation<Shape>> for AnyRelation {
    fn from(relation: Relation<Shape>) -> AnyRelation {
        AnyRelation::Interface(relation)
    }
}

impl From<Relation<MethodOrEntry>> for AnyRelation {
    fn from(relation: Relation<MethodOrEntry>) -> AnyRelation {
        AnyRelation::Method(Box::new(relation))
    }
}

impl From<Relation<Entry>> for AnyRelation {
    fn from(relation: Relation<Entry>) -> AnyRelation {
        AnyRelation::from(relation.map(|entry| MethodOrEntry::Entry(entry.clone())))
    }
}

impl From<Relation<MethodType>> for AnyRelation {
    fn from(relation: Relation<MethodType>) -> AnyRelation {
        AnyRelation::from(
            relation.map(|method_type| MethodOrEntry::MethodType(Box::new(method_type.clone()))),
        )
    }
}

impl From<Relation<Function>> for AnyRelation {
    fn from(relation: Relation<Function>) -> AnyRelation {
        AnyRelation::Function(Box::new(relation))
    }
}

impl From<Relation<Params>> for AnyRelation {
    fn from(relation: Relation<Params>) -> AnyRelation {
        AnyRelation::Params(Box::new(relation))
    }
}

impl From<Relation<Option<Block>>> for AnyRelation {
    fn from(relation: Relation<Option<Block>>) -> AnyRelation {
        AnyRelation::Block(Box::new(relation))
    }
}

impl From<Relation<Block>> for AnyRelation {
    fn from(relation: Relation<Block>) -> AnyRelation {
        AnyRelation::from(relation.map(|block| Some(block.clone())))
    }
}
