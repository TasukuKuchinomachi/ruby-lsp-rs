class UserTest
  def test_name
    User.new.name.no_such_method
  end
end
