rbs_version_argument = ARGV.find { |argument| argument.start_with?("--rbs-version=") }
rbs_version = rbs_version_argument ? ARGV.delete(rbs_version_argument).delete_prefix("--rbs-version=") : "4.2.0"
gem "rbs", rbs_version
require "rbs"

def ancestor(ancestor)
  case ancestor
  when RBS::Definition::Ancestor::Instance
    ancestor.args.empty? ? ancestor.name.to_s : "#{ancestor.name}[#{ancestor.args.join(", ")}]"
  when RBS::Definition::Ancestor::Singleton
    "singleton(#{ancestor.name})"
  end
end

def one_ancestors(one_ancestors)
  fields = {
    "super" => one_ancestors.super_class && [one_ancestors.super_class],
    "self_types" => one_ancestors.self_types,
    "include" => one_ancestors.included_modules,
    "include_interface" => one_ancestors.included_interfaces,
    "prepend" => one_ancestors.prepended_modules,
    "extend" => one_ancestors.extended_modules,
    "extend_interface" => one_ancestors.extended_interfaces
  }
  fields.filter_map { |name, ancestors| ancestors && "#{name}=[#{ancestors.map { ancestor(_1) }.join(", ")}]" }.join(" ")
end

def ancestors(ancestors)
  ancestors.ancestors.map { ancestor(_1) }.join(" < ")
end

loader = RBS::EnvironmentLoader.new
ARGV.each { |directory| loader.add(path: Pathname(directory)) }
environment = RBS::Environment.from_loader(loader).resolve_type_names
builder = RBS::DefinitionBuilder::AncestorBuilder.new(env: environment)

environment.class_decls.keys.sort_by(&:to_s).each do |name|
  puts name
  puts "  one instance: #{one_ancestors(builder.one_instance_ancestors(name))}"
  puts "  one singleton: #{one_ancestors(builder.one_singleton_ancestors(name))}"
  puts "  instance: #{ancestors(builder.instance_ancestors(name))}"
  puts "  singleton: #{ancestors(builder.singleton_ancestors(name))}"
end

environment.interface_decls.keys.sort_by(&:to_s).each do |name|
  puts name
  puts "  one interface: #{one_ancestors(builder.one_interface_ancestors(name))}"
  puts "  interface: #{ancestors(builder.interface_ancestors(name))}"
end
