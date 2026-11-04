value = ["x", nil].first

value && value.no_such_method

fallback = value || 1
fallback.no_such_method

both = value && value.size
both.no_such_method

value.nil? or value.no_such_method
value and value.no_such_method

if value && value.empty?
  value.no_such_method
end

if value.nil? || value.empty?
  value.no_such_method
else
  value.no_such_method
end
