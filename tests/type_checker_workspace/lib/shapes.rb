class Circle
  def area
    3 * radius
  end

  def radius
    2
  end
end

class Square
  def self.unit
    new
  end

  def area
    4
  end
end

circle = Circle.new
circle.area
square = Square.unit
square.area
"text".upcase
