class User
  attr_reader :name

  def initialize(name)
    @name = name
  end

  def greeting
    "Hello, #{name}"
  end
end

user = User.new("alice")
message = user.greeting
user.unknown_method
