gem "rbs", "4.2.0"
require "rbs"

def type_parameters(parameters)
  parameters.empty? ? "" : "[#{parameters.join(", ")}]"
end

def arguments(args)
  args.empty? ? "" : "[#{args.join(", ")}]"
end

def annotations(annotations)
  annotations.map { |annotation| "%a{#{annotation.string}} " }.join
end

def visibility(visibility)
  visibility ? "#{visibility} " : ""
end

def dump_declaration(declaration, depth)
  indent = "  " * depth
  case declaration
  when RBS::AST::Declarations::Class
    super_class = declaration.super_class&.then { |super_class| " < #{super_class.name}#{arguments(super_class.args)}" }
    puts "#{indent}#{annotations(declaration.annotations)}class #{declaration.name}#{type_parameters(declaration.type_params)}#{super_class}"
    dump_members(declaration.members, depth + 1)
    puts "#{indent}end"
  when RBS::AST::Declarations::Module
    self_types = declaration.self_types.empty? ? "" : " : #{declaration.self_types.map { |self_type| "#{self_type.name}#{arguments(self_type.args)}" }.join(", ")}"
    puts "#{indent}#{annotations(declaration.annotations)}module #{declaration.name}#{type_parameters(declaration.type_params)}#{self_types}"
    dump_members(declaration.members, depth + 1)
    puts "#{indent}end"
  when RBS::AST::Declarations::Interface
    puts "#{indent}#{annotations(declaration.annotations)}interface #{declaration.name}#{type_parameters(declaration.type_params)}"
    dump_members(declaration.members, depth + 1)
    puts "#{indent}end"
  when RBS::AST::Declarations::TypeAlias
    puts "#{indent}#{annotations(declaration.annotations)}type #{declaration.name}#{type_parameters(declaration.type_params)} = #{declaration.type}"
  when RBS::AST::Declarations::Constant, RBS::AST::Declarations::Global
    puts "#{indent}#{declaration.name}: #{declaration.type}"
  when RBS::AST::Declarations::ClassAlias
    puts "#{indent}class #{declaration.new_name} = #{declaration.old_name}"
  when RBS::AST::Declarations::ModuleAlias
    puts "#{indent}module #{declaration.new_name} = #{declaration.old_name}"
  end
end

def dump_members(members, depth)
  indent = "  " * depth
  members.each do |member|
    case member
    when RBS::AST::Members::MethodDefinition
      prefix = { instance: "", singleton: "self.", singleton_instance: "self?." }.fetch(member.kind)
      overloads = member.overloads.map { |overload| "#{annotations(overload.annotations)}#{overload.method_type}" }
      overloads << "..." if member.overloading?
      puts "#{indent}#{annotations(member.annotations)}#{visibility(member.visibility)}def #{prefix}#{member.name}: #{overloads.join(" | ")}"
    when RBS::AST::Members::AttrReader, RBS::AST::Members::AttrWriter, RBS::AST::Members::AttrAccessor
      keyword = { RBS::AST::Members::AttrReader => "attr_reader", RBS::AST::Members::AttrWriter => "attr_writer", RBS::AST::Members::AttrAccessor => "attr_accessor" }.fetch(member.class)
      prefix = member.kind == :singleton ? "self." : ""
      puts "#{indent}#{annotations(member.annotations)}#{visibility(member.visibility)}#{keyword} #{prefix}#{member.name}: #{member.type}"
    when RBS::AST::Members::Alias
      prefix = member.kind == :singleton ? "self." : ""
      puts "#{indent}#{annotations(member.annotations)}alias #{prefix}#{member.new_name} #{prefix}#{member.old_name}"
    when RBS::AST::Members::Include, RBS::AST::Members::Extend, RBS::AST::Members::Prepend
      keyword = { RBS::AST::Members::Include => "include", RBS::AST::Members::Extend => "extend", RBS::AST::Members::Prepend => "prepend" }.fetch(member.class)
      puts "#{indent}#{keyword} #{member.name}#{arguments(member.args)}"
    when RBS::AST::Members::InstanceVariable, RBS::AST::Members::ClassVariable
      puts "#{indent}#{member.name}: #{member.type}"
    when RBS::AST::Members::ClassInstanceVariable
      puts "#{indent}self.#{member.name}: #{member.type}"
    when RBS::AST::Members::Public
      puts "#{indent}public"
    when RBS::AST::Members::Private
      puts "#{indent}private"
    when RBS::AST::Declarations::Base
      dump_declaration(member, depth)
    end
  end
end

loader = RBS::EnvironmentLoader.new
ARGV.each { |directory| loader.add(path: Pathname(directory)) }
environment = RBS::Environment.from_loader(loader).resolve_type_names
rbs_gem_directory = Gem.loaded_specs.fetch("rbs").full_gem_path
environment.each_rbs_source do |source|
  puts "# #{source.buffer.name.to_s.delete_prefix("#{rbs_gem_directory}/")}"
  source.declarations.each { |declaration| dump_declaration(declaration, 0) }
end
