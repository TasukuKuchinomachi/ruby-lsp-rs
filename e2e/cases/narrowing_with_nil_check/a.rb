value = [1, nil].first

if value.nil?
  value.no_such_method
else
  value.no_such_method
end

if !value
  value.no_such_method
end

unless value.nil?
  value.no_such_method
end

value.nil? || value.no_such_method
