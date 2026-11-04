class Relay
  def target(*, **, &)
  end

  def positional(*)
    target(*)
  end

  def keywords(**)
    target(**)
  end

  def block(&)
    target(&)
  end
end

name = "x"
age = 1
record = { name:, age: }
record.no_such_method
