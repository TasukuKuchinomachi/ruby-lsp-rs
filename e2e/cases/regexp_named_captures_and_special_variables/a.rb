if /(?<word>\w+)/ =~ "hello"
  word.no_such_method
end

"hello" =~ /(l+)/
$1.no_such_method
$~.no_such_method
$&.no_such_method
$`.no_such_method
