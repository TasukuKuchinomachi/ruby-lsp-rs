queue = [1, 2, 3]

while (item = queue.shift)
  item.no_such_method
end

until queue.empty?
  queue.no_such_method
end

begin
  queue.pop
end while queue.size > 1

begin
  queue.pop
end until queue.empty?

loop_result = while false
end
loop_result.no_such_method

count = 0
count += 1 while count < 10
count -= 1 until count.zero?
