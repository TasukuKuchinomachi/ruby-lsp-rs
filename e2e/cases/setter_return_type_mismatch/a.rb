class Profile
  def name=(value)
    return if value.empty?
    @name = value
  end
end
