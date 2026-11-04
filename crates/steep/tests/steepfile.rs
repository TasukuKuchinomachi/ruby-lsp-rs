use std::{
    fs,
    path::{Path, PathBuf},
};

use steep::project::dsl;

#[test]
fn reads_the_steepfile_of_every_end_to_end_case() {
    let cases = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../e2e/cases");
    let mut read = 0;
    for entry in fs::read_dir(&cases).expect("e2e/cases is a directory") {
        let directory = entry.expect("the entry is readable").path();
        let steepfile = directory.join("Steepfile");
        let source = fs::read_to_string(&steepfile).expect("every case has a Steepfile");
        let project = dsl::parse(&directory, &source)
            .unwrap_or_else(|error| panic!("{}: {error}", steepfile.display()));
        assert!(
            !project.targets.is_empty(),
            "{} has no target",
            steepfile.display()
        );
        read += 1;
    }
    assert!(read > 0);
}

#[test]
fn lists_the_files_of_a_target_with_groups() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../e2e/cases/target_group");
    let source = fs::read_to_string(directory.join("Steepfile")).expect("the case has a Steepfile");
    let project = dsl::parse(&directory, &source).expect("the Steepfile is read");
    let target = &project.targets[0];
    assert_eq!(
        project.each_path_in_target(target),
        [
            PathBuf::from("lib/legacy/old_model.rb"),
            PathBuf::from("lib/model.rb"),
            PathBuf::from("sig/model.rbs"),
        ]
    );
}
