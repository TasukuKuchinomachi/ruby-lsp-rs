use serde::Serialize;

use crate::position::Range;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationLink {
    pub target_uri: String,
    pub target_range: Range,
    pub target_selection_range: Range,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum LocationOrLink {
    Location(Location),
    Link(LocationLink),
}

impl LocationOrLink {
    fn into_link(self) -> LocationLink {
        match self {
            LocationOrLink::Location(location) => LocationLink {
                target_uri: location.uri,
                target_range: location.range,
                target_selection_range: location.range,
            },
            LocationOrLink::Link(link) => link,
        }
    }
}

/// A response must be either `Location[]` or `LocationLink[]`, and clients such as Zed reject a mixed array. Locations
/// are turned into links rather than links into locations, because a link would lose its selection range
pub fn uniform_kinds(definitions: Vec<LocationOrLink>) -> Vec<LocationOrLink> {
    if definitions
        .iter()
        .all(|definition| matches!(definition, LocationOrLink::Location(_)))
    {
        return definitions;
    }
    definitions
        .into_iter()
        .map(|definition| LocationOrLink::Link(definition.into_link()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::Position;

    fn range(line: u32) -> Range {
        Range::new(Position::new(line, 0), Position::new(line, 5))
    }

    fn location(uri: &str, line: u32) -> LocationOrLink {
        LocationOrLink::Location(Location {
            uri: uri.to_string(),
            range: range(line),
        })
    }

    fn link(uri: &str, line: u32) -> LocationOrLink {
        LocationOrLink::Link(LocationLink {
            target_uri: uri.to_string(),
            target_range: range(line),
            target_selection_range: range(line),
        })
    }

    #[test]
    fn locations_mixed_with_links_become_links() {
        let definitions = uniform_kinds(vec![location("file:///a.rb", 7), link("file:///a.rbs", 11)]);
        assert_eq!(definitions, vec![link("file:///a.rb", 7), link("file:///a.rbs", 11)]);
    }

    #[test]
    fn locations_alone_stay_locations() {
        let definitions = vec![location("file:///a.rb", 7), location("file:///b.rb", 3)];
        assert_eq!(uniform_kinds(definitions.clone()), definitions);
    }
}
