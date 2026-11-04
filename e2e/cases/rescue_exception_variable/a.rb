begin
  Integer("x")
rescue ArgumentError, TypeError => error
  error.no_such_method
rescue => fallback
  fallback.no_such_method
end

class Job
  def run
    1
  rescue NoMatchingPatternError => error
    error.no_such_method
    2
  end
end
