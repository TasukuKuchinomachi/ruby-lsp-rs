steep_version_argument = ARGV.find { |argument| argument.start_with?("--steep-version=") }
steep_version = steep_version_argument ? ARGV.delete(steep_version_argument).delete_prefix("--steep-version=") : "2.1.0"
gem "rbs", "4.2.0"
gem "steep", steep_version
require "steep"

implicitly_returns_nil = !ARGV.delete("--implicitly-returns-nil").nil?

loader = RBS::EnvironmentLoader.new
ARGV.each { |directory| loader.add(path: Pathname(directory)) }
environment = RBS::Environment.from_loader(loader).resolve_type_names
definition_builder = RBS::DefinitionBuilder.new(env: environment)
factory = Steep::AST::Types::Factory.new(builder: definition_builder)
shape_builder = Steep::Interface::Builder.new(factory, implicitly_returns_nil: implicitly_returns_nil)

object_type = Steep::AST::Types::Name::Instance.new(name: RBS::TypeName.parse("::Object"), args: [])

def config(object_type, variable_bounds)
  Steep::Interface::Builder::Config.new(
    self_type: object_type,
    class_type: Steep::AST::Types::Name::Singleton.new(name: RBS::TypeName.parse("::Object")),
    instance_type: object_type,
    variable_bounds: variable_bounds
  )
end

def dump(shape_builder, header, type, config)
  shape = shape_builder.shape(type, config)
  unless shape
    puts "#{header}: no shape"
    puts
    return
  end

  puts "#{header}:"
  shape.methods.each_name.each do |method_name|
    entry = shape.methods[method_name] or next
    visibility = entry.private_method? ? "private" : "public"
    puts "  #{visibility} #{method_name}: #{entry.method_types.map(&:to_s).join(" | ")}"
  end
  puts
end

environment.class_decls.keys.sort_by(&:to_s).each do |type_name|
  entry = environment.class_decls.fetch(type_name)
  arguments = entry.type_params.map { |parameter| Steep::AST::Types::Var.new(name: parameter.name) }
  [
    Steep::AST::Types::Name::Instance.new(name: type_name, args: arguments),
    Steep::AST::Types::Name::Singleton.new(name: type_name)
  ].each do |type|
    dump(shape_builder, type.to_s, type, config(object_type, {}))
  end
end

environment.type_alias_decls.keys.sort_by(&:to_s).each do |type_name|
  next unless type_name.to_s.start_with?("::ShapeFixture::")

  declaration = environment.type_alias_decls.fetch(type_name).decl
  variable_bounds = declaration.type_params.each_with_object({}) do |parameter, bounds|
    bounds[parameter.name] = factory.type(parameter.upper_bound_type) if parameter.upper_bound_type
  end
  type = factory.type(declaration.type)
  dump(shape_builder, "type #{type_name} = #{type}", type, config(object_type, variable_bounds))
end

[Steep::AST::Types::Self.instance, Steep::AST::Types::Instance.instance, Steep::AST::Types::Class.instance].each do |type|
  dump(shape_builder, type.to_s, type, config(object_type, {}))
end
