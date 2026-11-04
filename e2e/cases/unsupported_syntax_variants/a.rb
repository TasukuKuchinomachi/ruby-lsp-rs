[1].each { |i| redo if i > 1 }

BEGIN { 1 }
END { 2 }

class Removed
  def target; end
  undef target
end

value = [1, "x"].first
case value
in Integer
  1
end

if (value == 1)..(value == 2)
  3
end
