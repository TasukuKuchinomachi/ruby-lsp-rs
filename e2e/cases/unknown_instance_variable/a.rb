class Account
  def rename(name)
    @name = name
    @nickname = name
    @nickname.upcase
  end
end
