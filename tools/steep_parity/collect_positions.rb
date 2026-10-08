require "json"
require "prism"

class PositionCollector < Prism::Visitor
  attr_reader :calls, :anchors

  def initialize(source)
    super()
    @source = source
    @line_starts = [0]
    source.each_byte.with_index { |byte, index| @line_starts << index + 1 if byte == 10 }
    @calls = []
    @anchors = []
    @block_depth = 0
  end

  def position(byte_offset)
    line = @line_starts.bsearch_index { |start| start > byte_offset }
    line = (line || @line_starts.size) - 1
    prefix = @source.byteslice(@line_starts[line], byte_offset - @line_starts[line]).force_encoding(Encoding::UTF_8)
    { "line" => line, "character" => prefix.encode(Encoding::UTF_16LE, invalid: :replace, undef: :replace).bytesize / 2 }
  end

  # Steep's locator treats a position at the very start of a token as also touching the preceding node, so the hover goes one character inside the name
  def inside_name(start_offset, end_offset)
    end_offset - start_offset >= 2 ? start_offset + 1 : start_offset
  end

  def add_anchor(kind, anchor_offset, name_start_offset, name_end_offset, name)
    @anchors << {
      "kind" => kind,
      "anchor" => position(anchor_offset),
      "hover" => position(inside_name(name_start_offset, name_end_offset)),
      "name" => name.to_s,
    }
  end

  def visit_call_node(node)
    message_location = node.message_loc
    if message_location && !@inside_implicit
      @calls << {
        "hover" => position(inside_name(message_location.start_offset, message_location.end_offset)),
        "name" => node.name.to_s,
      }
    end
    super
  end

  # `method(key:)` is a CallNode for `key` wrapped in an ImplicitNode; Steep's hover there resolves to the symbol key, not the call
  def visit_implicit_node(node)
    @inside_implicit = true
    super
    @inside_implicit = false
  end

  def visit_def_node(node)
    anchor_offsets = [node.name_loc.end_offset]
    anchor_offsets << node.rparen_loc.end_offset if node.rparen_loc
    anchor_offsets << node.parameters.location.end_offset if node.parameters
    anchor_offsets.uniq.each { |offset| add_anchor("method_return", offset, node.name_loc.start_offset, node.name_loc.end_offset, node.name) }
    saved_depth = @block_depth
    @block_depth = 0
    super
    @block_depth = saved_depth
  end

  def visit_block_node(node)
    @block_depth += 1
    super
    @block_depth -= 1
  end

  def visit_lambda_node(node)
    @block_depth += 1
    super
    @block_depth -= 1
  end

  %i[
    required_parameter_node optional_parameter_node rest_parameter_node required_keyword_parameter_node
    optional_keyword_parameter_node keyword_rest_parameter_node block_parameter_node
  ].each do |node_type|
    define_method(:"visit_#{node_type}") do |node|
      if @block_depth.positive? && node.respond_to?(:name_loc) && node.name_loc
        name_location = node.name_loc
        name_end = name_location.end_offset
        name_end -= 1 if @source.byteslice(name_end - 1, 1) == ":"
        add_anchor("block_parameter", name_end, name_location.start_offset, name_end, node.name)
      elsif @block_depth.positive? && node.is_a?(Prism::RequiredParameterNode)
        add_anchor("block_parameter", node.location.end_offset, node.location.start_offset, node.location.end_offset, node.name)
      end
      super(node)
    end
  end

  {
    local_variable_write_node: "local_variable",
    local_variable_or_write_node: "local_variable",
    local_variable_and_write_node: "local_variable",
    local_variable_operator_write_node: "local_variable",
    instance_variable_write_node: "instance_variable",
    instance_variable_or_write_node: "instance_variable",
    instance_variable_and_write_node: "instance_variable",
    instance_variable_operator_write_node: "instance_variable",
  }.each do |node_type, kind|
    define_method(:"visit_#{node_type}") do |node|
      add_anchor(kind, node.name_loc.end_offset, node.name_loc.start_offset, node.name_loc.end_offset, node.name)
      super(node)
    end
  end

  { local_variable_target_node: "local_variable", instance_variable_target_node: "instance_variable" }.each do |node_type, kind|
    define_method(:"visit_#{node_type}") do |node|
      add_anchor(kind, node.location.end_offset, node.location.start_offset, node.location.end_offset, node.name)
      super(node)
    end
  end
end

results = ARGV.to_h do |path|
  source = File.binread(path)
  collector = PositionCollector.new(source)
  Prism.parse(source).value.accept(collector)
  [path, { "calls" => collector.calls, "anchors" => collector.anchors }]
end
puts JSON.generate(results)
