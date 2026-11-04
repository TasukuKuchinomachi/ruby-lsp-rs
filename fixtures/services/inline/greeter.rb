class Greeter
  # @rbs @name: String

  #: (String) -> void
  def initialize(name)
    @name = name
  end

  # @rbs greeting: String
  # @rbs return: String
  def greet(greeting)
    "#{greeting}, #{@name}"
  end

  attr_reader :name #: String
end

Greeter.new("Steep").greet("Hello")
