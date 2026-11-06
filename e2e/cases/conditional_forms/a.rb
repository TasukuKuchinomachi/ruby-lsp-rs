value = [1, nil].first

unless value
  value.no_such_method
else
  value.no_such_method
end

ternary = value ? value.succ : "none"
ternary.no_such_method

modifier = value.succ if value
modifier.no_such_method

result = if value == 1
  :one
elsif value
  2.0
else
  nil
end
result.no_such_method
