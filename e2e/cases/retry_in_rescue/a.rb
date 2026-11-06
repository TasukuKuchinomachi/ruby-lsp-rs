attempts = 0

result = begin
  attempts += 1
  raise "failed" if attempts < 3
  "succeeded"
rescue RuntimeError
  retry
end

result.no_such_method
