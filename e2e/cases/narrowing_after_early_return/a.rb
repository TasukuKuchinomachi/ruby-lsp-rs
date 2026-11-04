class Finder
  def lookup(key)
    return 0 unless key
    key.no_such_method
    key.size
  end

  def strict(key)
    raise ArgumentError if key.nil?
    key.no_such_method
    key.size
  end
end
