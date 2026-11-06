rbs_version_argument = ARGV.find { |argument| argument.start_with?("--rbs-version=") }
rbs_version = rbs_version_argument ? ARGV.delete(rbs_version_argument).delete_prefix("--rbs-version=") : "4.2.0"
gem "rbs", rbs_version
require "rbs"

def annotations(annotations)
  "[#{annotations.map(&:string).join(", ")}]"
end

def dump_definition(kind, definition)
  type_parameters = definition.type_params_decl
  puts "#{definition.type_name} #{kind} self=#{definition.self_type} params=[#{kind == "singleton" ? "" : type_parameters.join(", ")}]"
  definition.methods.keys.sort.each do |name|
    method = definition.methods.fetch(name)
    super_method = method.super_method ? method.super_method.defined_in.to_s : "-"
    alias_of = method.alias_of ? method.alias_of.defined_in.to_s : "-"
    puts "  #{method.accessibility} #{name}: super=#{super_method} alias_of=#{alias_of} annotations=#{annotations(method.annotations)}"
    method.defs.each do |type_definition|
      puts "    #{type_definition.type} defined_in=#{type_definition.defined_in} implemented_in=#{type_definition.implemented_in || "-"} member=#{annotations(type_definition.member_annotations)} overload=#{annotations(type_definition.overload_annotations)}"
    end
  end
  definition.instance_variables.keys.sort.each do |name|
    variable = definition.instance_variables.fetch(name)
    puts "  #{name}: #{variable.type} declared_in=#{variable.declared_in}"
  end
  definition.class_variables.keys.sort.each do |name|
    variable = definition.class_variables.fetch(name)
    puts "  #{name}: #{variable.type} declared_in=#{variable.declared_in}"
  end
end

loader = RBS::EnvironmentLoader.new
ARGV.each { |directory| loader.add(path: Pathname(directory)) }
environment = RBS::Environment.from_loader(loader).resolve_type_names
builder = RBS::DefinitionBuilder.new(env: environment)

environment.class_decls.keys.sort_by(&:to_s).each do |name|
  dump_definition("instance", builder.build_instance(name))
  dump_definition("singleton", builder.build_singleton(name))
end

environment.interface_decls.keys.sort_by(&:to_s).each do |name|
  dump_definition("interface", builder.build_interface(name))
end
