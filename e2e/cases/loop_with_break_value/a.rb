found = loop do
  break 1
end
found.no_such_method

count = 0
without_break = loop do
  count += 1
  raise StopIteration if count > 2
end
without_break.no_such_method
