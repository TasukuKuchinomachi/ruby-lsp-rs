point = Point.new(1, 2)
point.move(dx: 1, dy: 2)
point.move(point)
point.distance(point, round: true)
point.each_coordinate { |coordinate| coordinate + 1 }
origin = Point.origin
label = origin.to_s.upcase
circle = Services::Shapes::Circle.new(point, 3)
circle.resize(1.5).area
circle.color
version = Services::VERSION
canvas = Canvas.new
canvas.shapes.map(&:area)
canvas.palette[:background]
# @type var maybe: Point?
maybe = nil
maybe&.x
numbers = [1, 2, 3] #: Array[Integer]
numbers.each_slice(2).to_a
[point, origin].sum(&:x)
