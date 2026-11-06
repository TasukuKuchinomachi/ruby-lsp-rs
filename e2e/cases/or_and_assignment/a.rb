class Settings
  # @dynamic label, label=
  attr_accessor :label

  def load
    local = [nil, "x"].first
    local ||= "default"
    local.no_such_method

    @name ||= "default"
    @name.no_such_method
    @name ||= 1

    $cache ||= "default"
    $cache &&= $cache.upcase
    $cache &&= 2

    self.label ||= "label"
    self.label ||= 3

    table = { "a" => 1 }
    table["b"] ||= 2
    table["c"] ||= "three"

    flag = true
    flag &&= :symbol
    flag.no_such_method
  end
end
