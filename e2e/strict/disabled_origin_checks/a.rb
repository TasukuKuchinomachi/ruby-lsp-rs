class Inventory
  def origins
    each_untyped { |name| name.upcase }
    begin
      anything
    rescue => error
      error.message
    end
    counter = -> (count) { count.succ }
    nil
  end
end
