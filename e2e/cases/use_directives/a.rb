class App
  def circle
    "not a circle"
  end

  def box
    Shapes::Square.new
  end

  def leaf
    Shapes::Circle.new
  end
end

App.new.circle.radius.no_such_method
App.new.box.side.no_such_method
