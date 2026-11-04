value = [1, "x"].first

value => Integer => matched
matched.no_such_method

found = (value in String)
found.no_such_method

case value
in Integer
  value.no_such_method
end

def forward_rest(*, **)
  Kernel.puts(*)
  { **, key: 1 }
end
