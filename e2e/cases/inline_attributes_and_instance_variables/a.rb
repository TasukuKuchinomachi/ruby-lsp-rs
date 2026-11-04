class Person
  # @rbs @age: Integer

  attr_reader :name #: String
  attr_accessor :nickname #: String?

  #: (String, Integer) -> void
  def initialize(name, age)
    @name = name
    @age = age
    @age = "old"
  end

  #: () -> Integer
  def age
    @age
  end
end

person = Person.new("x", 1)
person.name.no_such_method
person.nickname.no_such_method
person.nickname = 1
