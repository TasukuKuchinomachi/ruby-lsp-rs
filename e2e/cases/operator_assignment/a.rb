class Counter
  # @dynamic size, size=
  attr_accessor :size

  def increment
    local = 1
    local += 1
    local += "one"

    @count += 1
    @count -= "one"

    @@instances += 1
    @@instances *= "two"

    $total += 1
    $total += :three

    self.size += 1
    self.size += "four"

    numbers = [1, 2]
    numbers[0] += 1
    numbers[1] += "five"
  end
end
