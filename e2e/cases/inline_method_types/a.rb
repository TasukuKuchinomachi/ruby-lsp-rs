class Calculator
  #: (Integer, Integer) -> Integer
  def add(left, right)
    left + right
  end

  #: (Integer) -> String
  def describe(value)
    value
  end

  # @rbs value: Integer
  # @rbs return: String
  def label(value)
    value.no_such_method
    value.to_s
  end

  # @rbs *values: Integer
  # @rbs **options: String
  # @rbs &block: (Integer) -> void
  # @rbs return: void
  def each_value(*values, **options, &block)
    values.no_such_method
    options.no_such_method
  end

  def untyped_method(value)
    value.no_such_method
  end
end

Calculator.new.add(1, "2")
Calculator.new.label(1).no_such_method
