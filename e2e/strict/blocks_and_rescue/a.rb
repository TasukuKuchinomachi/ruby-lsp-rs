class Inventory
  def block_parameters
    items { |count| count.succ }
    each_untyped { |name, _| name.no_such_method }
    each_value { |value| value.no_such_method }
    each_value { |key, value| key.no_such_method }
    legacy(1) { |argument| argument.no_such_method }
    items { |count, size: 1| size.no_such_method }
    pairs { |number, *rest, last| last.no_such_method }
  end

  def lambdas
    typed = -> (count) { count.succ } #: ^(Integer) -> Integer
    untyped = -> (count) { count.no_such_method }
    empty = -> {}
    numbered = -> { _1.no_such_method }
  end

  def jumps
    items { |count| next count.to_s }
    [1].map { |count| break count.to_s }
    [1].each { |count| next if count.zero? }
    -> (count) { next count.no_such_method }
    -> (count) { break count.no_such_method } #: ^(Integer) -> void
  end

  def rescues
    begin
      anything
    rescue => error
      error.no_such_method
    rescue
      nil
    end

    begin
      anything
    rescue StandardError => error
      error.no_such_method
    rescue ArgumentError, error_class => error
      error.no_such_method
    rescue GenericError => error
      error.no_such_method
    rescue => @error
      nil
    end
  end

  def loops
    for count in [1]
      count.no_such_method
    end
    for value in anything
      value.no_such_method
    end
    for item in self
      item.no_such_method
    end
  end

  def multiple_assignments
    first, second = [1, "a"]
    first.no_such_method
    third, fourth = anything
    third.no_such_method
    (fifth, sixth), seventh = [anything, 1]
    fifth.no_such_method
  end
end
