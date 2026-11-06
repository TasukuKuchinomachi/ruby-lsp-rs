class Point
  # @dynamic x, x=, y, y=
  attr_accessor :x, :y
end

point = Point.new
point.x, point.y = 1, "two"

numbers = [0, 0]
numbers[0], numbers[1] = 1, :two

LIMIT ||= 10
LIMIT ||= "ten"
