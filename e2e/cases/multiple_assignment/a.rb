a, b = 1, "two"
a.no_such_method
b.no_such_method

c, (d, e) = [1, [:three, 4.0]]
c.no_such_method
d.no_such_method
e.no_such_method

first, *rest = [1, 2, 3]
first.no_such_method
rest.no_such_method

*init, last = "a", "b", :c
init.no_such_method
last.no_such_method

x = 1
y = "y"
x, y = y, x
x.no_such_method

class Pair
  def assign(tuple)
    @left, @right = tuple
    @left, @right = @right, @left
  end
end

# @type var number: Integer
number, text = "wrong", "text"
