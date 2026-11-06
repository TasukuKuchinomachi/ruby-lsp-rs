# @type var status: status
status = :active

if status == :active
  status.no_such_method
else
  status.no_such_method
end

case status
when :active, :inactive
  status.no_such_method
else
  status.no_such_method
end
