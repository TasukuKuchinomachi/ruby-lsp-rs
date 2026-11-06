value = [1, "x", :y].first

if value.is_a?(Integer)
  value.no_such_method
else
  value.no_such_method
end

if value.kind_of?(String)
  value.no_such_method
end

if value.instance_of?(Symbol)
  value.no_such_method
end

unless value.is_a?(Integer)
  value.no_such_method
end
