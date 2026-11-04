class Calculator
  # @rbs (Integer, Integer) -> Integer
  #    | (Float, Float) -> Float
  def add(left, right) = left + right

  #: (Integer) -> Integer
  #: (String) -> String
  def double(value) = value * 2

  def negate(value) #: Integer
    "negated"
  end
end

class PreciseCalculator < Calculator
  # @rbs (Rational, Rational) -> Rational | ...
  def add(left, right) = super
end

Calculator.new.add(1, 2.0)
Calculator.new.double(:symbol)
PreciseCalculator.new.add(1r, 2r).no_such_method
