class User
  attr_reader :name

  def initialize(name)
    @name = name
  end

  def greeting
    "Hello, #{name}"
  end

  def format(width, padding = " ")
    greeting.center(width, padding)
  end
end

user = User.new("alice")
message = user.greeting
user.unknown_method
user.format(10, "-")
