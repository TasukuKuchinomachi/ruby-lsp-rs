class InlineCalculator
  # @rbs (Integer, Integer) -> Integer
  def add(x, y) = x + y

  # @rbs @unused: Integer
  def unused = 1
end

InlineCalculator.new.add(1, "2")
