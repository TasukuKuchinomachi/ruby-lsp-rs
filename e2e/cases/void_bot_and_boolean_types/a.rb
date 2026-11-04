worker = Worker.new

result = worker.perform
result.no_such_method

flag = worker.ready?
flag.no_such_method

value = [1, nil].first
value = worker.fail! unless value
value.no_such_method

unreachable = worker.fail!
unreachable.no_such_method
