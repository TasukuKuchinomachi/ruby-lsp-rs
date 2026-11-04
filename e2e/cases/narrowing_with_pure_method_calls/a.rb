order = Order.new

if order.note
  order.note.no_such_method
end

if order.coupon
  order.coupon.no_such_method
end

list = ["a", nil]
if list[0]
  list[0].no_such_method
end

table = { a: 1, b: nil }
if table[:b]
  table[:b].no_such_method
end
