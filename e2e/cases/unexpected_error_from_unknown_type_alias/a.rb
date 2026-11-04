class Item
  def member
    1
  end

  def accept(member)
    member.foo
  end

  def name
    "item"
  end
end

item = Item.new
value = item.member
value.foo
item.accept(1)
item.name.bar
