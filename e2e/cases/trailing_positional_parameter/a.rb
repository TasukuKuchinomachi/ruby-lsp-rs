class Formatter
  def wrap(open, *middle, close)
    open + middle.join + close
  end
end

Formatter.new.wrap("(", "a", ")")
Formatter.new.wrap("(", 1, ")")
