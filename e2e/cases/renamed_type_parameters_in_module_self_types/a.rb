module Walker
  def first
    walk.no_such_method
  end

  def last
    walk
  end
end
