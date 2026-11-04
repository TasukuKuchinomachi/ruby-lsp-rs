use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::LazyLock,
};

use regex::Regex;

use rbs_definition::{ast::Declaration, location::Location};
use steep::{
    environment_loader,
    project::dsl,
    ruby::node::{Buffer, SourceRange},
    services::type_check_service::TypeCheckService,
    steep_version::SteepVersion,
    strict::EnabledChecks,
};

pub enum Json {
    Null,
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn string(value: impl Into<String>) -> Json {
        Json::String(value.into())
    }

    pub fn optional_string(value: Option<impl Into<String>>) -> Json {
        value.map_or(Json::Null, |value| Json::String(value.into()))
    }

    pub fn object<const SIZE: usize>(fields: [(&str, Json); SIZE]) -> Json {
        Json::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    pub fn write(&self, output: &mut String) {
        match self {
            Json::Null => output.push_str("null"),
            Json::String(value) => write_string(value, output),
            Json::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    value.write(output);
                }
                output.push(']');
            }
            Json::Object(fields) => {
                output.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    write_string(key, output);
                    output.push(':');
                    value.write(output);
                }
                output.push('}');
            }
        }
    }
}

fn write_string(value: &str, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
            '\u{0}'..='\u{1f}' => output.push_str(&format!("\\u{:04x}", character as u32)),
            _ => output.push(character),
        }
    }
    output.push('"');
}

pub struct Query {
    pub line: usize,
    pub column: usize,
    pub position: usize,
}

pub fn query_positions(text: &str) -> Vec<Query> {
    let characters: Vec<(usize, char)> = text.char_indices().collect();
    let is_word = |character: char| character.is_ascii_alphanumeric() || character == '_';
    let mut indexes = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        if is_word(characters[index].1) {
            let start = index;
            while index < characters.len() && is_word(characters[index].1) {
                index += 1;
            }
            indexes.push(start);
            indexes.push(index);
        } else {
            index += 1;
        }
    }
    for (index, (_, character)) in characters.iter().enumerate() {
        if matches!(character, '.' | ':' | '@' | '(' | ',' | '$') {
            indexes.push(index + 1);
        }
    }
    indexes.sort();
    indexes.dedup();
    let mut queries = Vec::new();
    for index in indexes {
        let position = characters
            .get(index)
            .map_or(text.len(), |(position, _)| *position);
        let before = &text[..position];
        let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
        queries.push(Query {
            line: before.matches('\n').count() + 1,
            column: before[line_start..].chars().count(),
            position,
        });
    }
    queries
}

pub struct Dump {
    pub output: String,
    pub directory: PathBuf,
}

// Why not print the numbers of fresh type variables: Steep counts them in the whole
// process, so they depend on the queries made before.
static FRESH_VARIABLE_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\w)\(\d+\)").expect("the pattern is valid"));

impl Dump {
    pub fn print(&mut self, path: &Path, query: &Query, result: Json) {
        self.output.push_str(&format!(
            "{}:{}:{}: ",
            self.directory.join(path).display(),
            query.line,
            query.column
        ));
        let mut json = String::new();
        result.write(&mut json);
        self.output
            .push_str(&FRESH_VARIABLE_NUMBER.replace_all(&json, "${1}(*)"));
        self.output.push('\n');
    }
}

pub fn ruby_range(path: &Path, buffer: &Buffer, range: SourceRange) -> Json {
    Json::String(format!(
        "{}:{}:{}-{}:{}",
        path.display(),
        buffer.line(range.begin_pos),
        buffer.column(range.begin_pos),
        buffer.line(range.end_pos),
        buffer.column(range.end_pos)
    ))
}

pub fn rbs_location(location: Option<&Location>) -> Json {
    match location {
        Some(location) => Json::String(format!(
            "{}:{}:{}-{}:{}",
            location.name(),
            location.start_line(),
            location.start_column(),
            location.end_line(),
            location.end_column()
        )),
        None => Json::Null,
    }
}

pub fn declaration(declaration: &Declaration) -> Json {
    let (kind, name, location) = match declaration {
        Declaration::Class(class) => (
            if class.declared_in_ruby {
                "Declarations::ClassDecl"
            } else {
                "Declarations::Class"
            },
            class.name.to_string(),
            class.location.as_ref(),
        ),
        Declaration::Module(module) => (
            if module.declared_in_ruby {
                "Declarations::ModuleDecl"
            } else {
                "Declarations::Module"
            },
            module.name.to_string(),
            module.location.as_ref(),
        ),
        Declaration::Interface(interface) => (
            "Declarations::Interface",
            interface.name.to_string(),
            interface.location.as_ref(),
        ),
        Declaration::TypeAlias(alias) => (
            "Declarations::TypeAlias",
            alias.name.to_string(),
            alias.location.as_ref(),
        ),
        Declaration::Constant(constant) => (
            if constant.declared_in_ruby {
                "Declarations::ConstantDecl"
            } else {
                "Declarations::Constant"
            },
            constant.name.to_string(),
            constant.location.as_ref(),
        ),
        Declaration::Global(global) => (
            "Declarations::Global",
            global.name.clone(),
            global.location.as_ref(),
        ),
        Declaration::ClassAlias(alias) => (
            match (alias.declared_in_ruby, alias.kind) {
                (true, _) => "Declarations::ClassModuleAliasDecl",
                (false, rbs_definition::ast::ClassOrModule::Class) => "Declarations::ClassAlias",
                (false, rbs_definition::ast::ClassOrModule::Module) => "Declarations::ModuleAlias",
            },
            alias.new_name.to_string(),
            alias.location.as_ref(),
        ),
    };
    Json::object([
        ("kind", Json::string(kind)),
        ("name", Json::String(name)),
        ("location", rbs_location(location)),
    ])
}

pub struct Arguments {
    pub steep_version: SteepVersion,
    pub directories: Vec<PathBuf>,
}

pub fn arguments() -> Result<Arguments, String> {
    let mut steep_version = SteepVersion::default();
    let mut directories = Vec::new();
    for argument in env::args().skip(1) {
        if let Some(version) = argument.strip_prefix("--steep-version=") {
            steep_version = SteepVersion::parse(version)
                .ok_or_else(|| format!("unsupported Steep version: {version}"))?;
        } else {
            directories.push(PathBuf::from(argument));
        }
    }
    Ok(Arguments {
        steep_version,
        directories,
    })
}

pub struct Project {
    pub service: TypeCheckService,
    pub paths: Vec<PathBuf>,
}

pub fn load_project(directory: &Path, steep_version: SteepVersion) -> Result<Project, String> {
    let base_directory =
        fs::canonicalize(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let steepfile = base_directory.join("Steepfile");
    let content = fs::read_to_string(&steepfile)
        .map_err(|error| format!("{}: {error}", steepfile.display()))?;
    let project = dsl::parse(&base_directory, &content)?;
    let rbs_gem = environment_loader::rbs_gem(None)?;
    let mut service = TypeCheckService::load(
        project,
        &rbs_gem,
        steep_version,
        EnabledChecks::default(),
        1,
    )?;
    service.load_files()?;
    let mut paths = service.project_paths();
    paths.sort();
    Ok(Project { service, paths })
}

pub fn run(each: impl Fn(&mut Project, &Path, &str, &mut Dump)) -> ExitCode {
    let arguments = match arguments() {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let mut dump = Dump {
        output: String::new(),
        directory: PathBuf::new(),
    };
    for directory in &arguments.directories {
        dump.directory = directory.clone();
        let mut project = match load_project(directory, arguments.steep_version) {
            Ok(project) => project,
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::FAILURE;
            }
        };
        for path in project.paths.clone() {
            let absolute_path = project.service.project.absolute_path(&path);
            let Ok(text) = fs::read_to_string(&absolute_path) else {
                continue;
            };
            each(&mut project, &path, &text, &mut dump);
        }
    }
    print!("{}", dump.output);
    ExitCode::SUCCESS
}
