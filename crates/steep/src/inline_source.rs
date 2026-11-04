use std::path::PathBuf;

use rbs_definition::{collections::HashSet, inline_parser, location};

use crate::{
    diagnostic::configuration::Severity,
    expectations,
    project::{Project, Target},
    ruby::node::{Buffer, SourceRange},
    type_check_service::lsp_range,
};

fn project_paths(project: &Project) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut seen = HashSet::default();
    for target in &project.targets {
        for path in project.each_path_in_target(target) {
            if seen.insert(path.clone()) {
                paths.push(path);
            }
        }
    }
    paths
}

fn inline_paths(project: &Project) -> Vec<(PathBuf, &Target)> {
    project_paths(project)
        .into_iter()
        .filter(|path| project.target_for_signature_path(path).is_none())
        .filter_map(|path| {
            let target = project.target_for_inline_source_path(&path)?;
            Some((path, target))
        })
        .collect()
}

pub fn signature_paths_for(project: &Project, target: &Target) -> Vec<PathBuf> {
    inline_paths(project)
        .into_iter()
        .filter(|(_, owner)| owner.name == target.name || !owner.unreferenced)
        .map(|(path, _)| path)
        .collect()
}

pub fn target_inline_paths(project: &Project, target: &Target) -> Vec<PathBuf> {
    inline_paths(project)
        .into_iter()
        .filter(|(_, owner)| owner.name == target.name)
        .map(|(path, _)| path)
        .collect()
}

pub fn inline_diagnostics(buffer: &Buffer) -> Vec<expectations::Diagnostic> {
    let signature_buffer = location::Buffer::new(buffer.name.clone(), buffer.source.clone());
    inline_parser::parse(&signature_buffer)
        .diagnostics
        .into_iter()
        .map(|diagnostic| expectations::Diagnostic {
            range: lsp_range(
                buffer,
                SourceRange {
                    begin_pos: diagnostic.location.start,
                    end_pos: diagnostic.location.end,
                },
            ),
            severity: Severity::Error.expectation_name().to_string(),
            message: diagnostic.message,
            code: "RBS::InlineDiagnostic".to_string(),
        })
        .collect()
}
