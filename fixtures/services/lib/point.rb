class Point
  attr_reader :x, :y

  def initialize(x, y)
    @x = x
    @y = y
    @label = nil
  end

  def move(dx: 0, dy: 0)
    Point.new(x + dx, y + dy)
  end

  def distance(other, round: false, **options)
    Math.sqrt((x - other.x)**2 + (y - other.y)**2)
  end

  def each_coordinate
    yield x
    yield y
  end

  def self.origin
    new(0, 0)
  end

  def to_s
    "(#{x}, #{y})"
  end
end
