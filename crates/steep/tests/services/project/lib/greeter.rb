class Greeter
  def initialize(name)
    @name = name
  end

  def greet(greeting, times: 1)
    greeting + @name
  end
end
