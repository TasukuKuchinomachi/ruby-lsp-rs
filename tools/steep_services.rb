steep_version_argument = ARGV.find { |argument| argument.start_with?("--steep-version=") }
steep_version = steep_version_argument ? ARGV.delete(steep_version_argument).delete_prefix("--steep-version=") : "2.1.0"
rbs_version_argument = ARGV.find { |argument| argument.start_with?("--rbs-version=") }
rbs_version = rbs_version_argument ? ARGV.delete(rbs_version_argument).delete_prefix("--rbs-version=") : "4.2.0"
gem "rbs", rbs_version
gem "steep", steep_version
require "steep"
require "json"
require "zlib"

Steep.log_output = File.open(File::NULL, "w")

module SteepServices
  module_function

  def load_project(directory)
    steepfile = Pathname(directory).realpath + "Steepfile"
    project = Steep::Project.new(steepfile_path: steepfile)
    Steep::Project::DSL.parse(project, steepfile.read, filename: steepfile.to_s)
    project
  end

  def project_paths(project)
    loader = Steep::Services::FileLoader.new(base_dir: project.base_dir)
    paths = []
    project.targets.each do |target|
      loader.each_path_in_target(target) { |path| paths << path unless paths.include?(path) }
    end
    paths
  end

  def load_service(project, paths)
    service = Steep::Services::TypeCheckService.new(project: project)
    changes = paths.to_h { |path| [path, [Steep::Services::ContentChange.string((project.base_dir + path).read)]] }
    service.update(changes: changes)
    service
  end

  def query_positions(text)
    indexes = []
    text.scan(/[A-Za-z0-9_]+/) do
      match = Regexp.last_match
      indexes << match.begin(0) << match.end(0)
    end
    text.scan(/[.:@(,$]/) { indexes << Regexp.last_match.end(0) }
    indexes.uniq.sort.map do |index|
      before = text[0...index]
      line_start = (before.rindex("\n") || -1) + 1
      [before.count("\n") + 1, index - line_start]
    end
  end

  def each_query(directories)
    directories.each do |directory|
      project = load_project(directory)
      paths = project_paths(project).sort
      service = load_service(project, paths)
      yield project, service, paths, directory
    end
  end

  def ruby_range(range)
    return nil unless range

    "#{range.source_buffer.name}:#{range.line}:#{range.column}-#{range.last_line}:#{range.last_column}"
  end

  def rbs_location(location)
    return nil unless location

    "#{location.buffer.name}:#{location.start_line}:#{location.start_column}-#{location.end_line}:#{location.end_column}"
  end

  def location(location)
    case location
    when RBS::Location then rbs_location(location)
    when Parser::Source::Range then ruby_range(location)
    when nil then nil
    else raise "unexpected location: #{location.inspect}"
    end
  end

  def position(position)
    "#{position.line}:#{position.column}"
  end

  def comment(comment)
    comment&.string
  end

  def declaration(declaration)
    return nil unless declaration

    name =
      case declaration
      when RBS::AST::Declarations::ClassAlias, RBS::AST::Declarations::ModuleAlias then declaration.new_name
      when RBS::AST::Ruby::Declarations::ClassModuleAliasDecl then declaration.new_name
      when RBS::AST::Ruby::Declarations::ClassDecl then declaration.class_name
      when RBS::AST::Ruby::Declarations::ModuleDecl then declaration.module_name
      when RBS::AST::Ruby::Declarations::ConstantDecl then declaration.constant_name
      else declaration.name
      end
    {
      "kind" => declaration.class.name.split("::").last(2).join("::"),
      "name" => name.to_s,
      "location" => location(declaration.location)
    }
  end

  def member_location(member)
    case member
    when RBS::AST::Members::Base then location(member.location)
    when RBS::AST::Ruby::Members::Base then location(member.location)
    end
  end

  # Why not print the numbers of fresh type variables and the names of the variables
  # of case-when: Steep counts the former in the whole process and makes the latter
  # random.
  def normalize(json)
    json.gsub(/(?<=\w)\(\d+\)/, "(*)").gsub(/__case_when:\w{5}__/, "__case_when:*__")
  end

  # Why not print every item: a completion at the start of a statement lists all the
  # constants and methods, so printing them at every position makes gigabytes. Set
  # STEEP_SERVICES_FULL=1 to print them.
  def summarize(values)
    lines = values.map { |value| normalize(JSON.generate(value)) }.sort
    return lines.map { |line| JSON.parse(line) } if ENV["STEEP_SERVICES_FULL"]

    { "count" => lines.size, "crc32" => Zlib.crc32(lines.join("\n")).to_s(16) }
  end

  def print_result(directory, path, line, column, result)
    puts "#{Pathname(directory) + path}:#{line}:#{column}: #{normalize(JSON.generate(result))}"
  end
end
