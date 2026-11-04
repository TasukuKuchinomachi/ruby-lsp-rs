require "prism"

steepfile, preset = ARGV
unless steepfile && %w[all_error strict default lenient silent].include?(preset)
  abort "Usage: ruby override_diagnostics.rb STEEPFILE all_error|strict|default|lenient|silent"
end

source = File.binread(steepfile)
result = Prism.parse(source)
abort result.errors.map(&:message).join("\n") unless result.success?

class BlockEndFinder < Prism::Visitor
  attr_reader :closing_locations

  def initialize
    @closing_locations = []
    super
  end

  def visit_call_node(node)
    if node.receiver.nil? && %i[target group].include?(node.name) && node.block.is_a?(Prism::BlockNode)
      closing_locations << node.block.closing_loc
    end
    super
  end
end

finder = BlockEndFinder.new
result.value.accept(finder)
abort "#{steepfile} has no target block" if finder.closing_locations.empty?

call = "configure_code_diagnostics(Steep::Diagnostic::Ruby.#{preset})"
finder.closing_locations.sort_by(&:start_offset).reverse_each do |closing|
  line_start = closing.start_offset - closing.start_column
  indentation = source.byteslice(line_start, closing.start_column)
  if indentation.strip.empty?
    source.insert(line_start, "#{indentation}  #{call}\n")
  else
    source.insert(closing.start_offset, "\n#{call}\n")
  end
end

File.binwrite(steepfile, source)
puts "#{steepfile}: configure_code_diagnostics(Steep::Diagnostic::Ruby.#{preset}) at the end of #{finder.closing_locations.size} target and group blocks"
