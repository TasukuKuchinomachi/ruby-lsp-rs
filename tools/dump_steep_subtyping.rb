steep_version_argument = ARGV.find { |argument| argument.start_with?("--steep-version=") }
steep_version = steep_version_argument ? ARGV.delete(steep_version_argument).delete_prefix("--steep-version=") : "2.1.0"
gem "rbs", "4.2.0"
gem "steep", steep_version
require "steep"

Steep.logger.level = Logger::FATAL

loader = RBS::EnvironmentLoader.new
ARGV.each { |directory| loader.add(path: Pathname(directory)) }
environment = RBS::Environment.from_loader(loader).resolve_type_names
definition_builder = RBS::DefinitionBuilder.new(env: environment)
factory = Steep::AST::Types::Factory.new(builder: definition_builder)
shape_builder = Steep::Interface::Builder.new(factory, implicitly_returns_nil: false)
checker = Steep::Subtyping::Check.new(builder: shape_builder)

def kind(relation)
  case
  when relation.interface? then "interface"
  when relation.method? then "method"
  when relation.function? then "function"
  when relation.params? then "params"
  when relation.block? then "block"
  else "type"
  end
end

def relation_string(relation)
  if relation.interface?
    "#{relation.sub_type.type} <: #{relation.super_type.type}"
  else
    relation.to_s
  end
end

def relation_message(relation)
  case
  when relation.type?
    relation.to_s
  when relation.method?
    if relation.super_type.is_a?(Steep::Interface::MethodType) && relation.sub_type.is_a?(Steep::Interface::MethodType)
      relation.to_s
    end
  when relation.interface?
    nil
  when relation.block?
    "(Blocks are incompatible)"
  when relation.function?
    nil
  when relation.params?
    "(Params are incompatible)"
  end
end

def result_line(result)
  line = "#{result.class.name.split("::").last} #{kind(result.relation)}: #{relation_string(result.relation)}"
  if result.is_a?(Steep::Subtyping::Result::Failure)
    line += " (#{result.error.class.name.split("::").last}: #{result.error.message})"
  end
  line
end

def write_tree(result, depth)
  puts "#{"  " * depth}#{result_line(result)}"
  case result
  when Steep::Subtyping::Result::Expand
    write_tree(result.child, depth + 1)
  when Steep::Subtyping::Result::All, Steep::Subtyping::Result::Any
    result.branches.each { |branch| write_tree(branch, depth + 1) }
  end
end

def write_result(result)
  puts "  #{result.success? ? "success" : "failure"}"
  puts "  tree:"
  write_tree(result, 2)
  return if result.success?

  puts "  failure path:"
  (result.failure_path || []).reverse_each do |step|
    puts "    #{result_line(step)}"
  end
  puts "  diagnostic:"
  (result.failure_path || []).reverse_each.filter_map { |step| relation_message(step.relation) }.each.with_index(1) do |message, index|
    puts "  #{"  " * index}#{message}"
  end
end

environment.class_decls.keys.select { |name| name.to_s.start_with?("::SubtypingFixture::") }.sort_by(&:to_s).each do |type_name|
  next unless type_name.to_s.end_with?("Relations")

  class_params = environment.class_decls.fetch(type_name).type_params.map { |param| factory.type_param(param) }
  self_type = Steep::AST::Types::Name::Instance.new(
    name: type_name,
    args: class_params.map { |param| Steep::AST::Types::Var.new(name: param.name) }
  )
  class_type = Steep::AST::Types::Name::Singleton.new(name: type_name)
  definition = definition_builder.build_instance(type_name)

  definition.methods.each do |method_name, method|
    next unless method.defs.first.defined_in == type_name

    method_type = factory.method_type(method.defs.first.type)
    relation = Steep::Subtyping::Relation.new(
      sub_type: method_type.type.params.required.first,
      super_type: method_type.type.return_type
    )
    variables = method_type.type_params.map(&:name)
    puts "#{type_name}##{method_name}: #{relation}"

    begin
      checker.push_variable_bounds(class_params + method_type.type_params) do
        constraints = Steep::Subtyping::Constraints.new(unknowns: variables)
        result = checker.check(relation, constraints: constraints, self_type: self_type, instance_type: self_type, class_type: class_type)
        write_result(result)
        puts "  constraints: #{constraints}"
        next if variables.empty?

        begin
          solution = constraints.solution(
            checker,
            variance: Steep::Subtyping::VariableVariance.from_method_type(method_type),
            variables: variables,
            self_type: self_type,
            instance_type: self_type,
            class_type: class_type
          )
          puts "  solution: #{solution.dictionary.map { |variable, type| "#{variable} => #{type}" }.join(", ")}"
        rescue Steep::Subtyping::Constraints::UnsatisfiableConstraint => error
          puts "  unsatisfiable: #{error.var}: #{error.sub_type} <: #{error.super_type}"
        end
      end
    rescue StandardError
      puts "  error"
    end
    puts
  end
end
